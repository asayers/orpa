use crate::{db_path, GitlabConfig, Version, VersionInfo};
use anyhow::anyhow;
use git2::{Oid, Repository};
use gitlab::{Gitlab, MergeRequest, MergeRequestInternalId, MergeRequestState, ProjectId};
use std::collections::HashSet;
use std::fs::File;
use tracing::*;

pub fn fetch(repo: &Repository) -> anyhow::Result<()> {
    let config = GitlabConfig::load(repo)?;

    info!("Opening the database");
    let db_path = db_path(repo);
    let db = crate::mr_db::Db::open(&db_path)?;

    info!("Connecting to gitlab at {}", config.host);
    let gl = Gitlab::new(&config.host, &config.token)?;

    println!("Fetching open MRs for project {}...", config.project_id);
    let mrs: Vec<MergeRequest> = {
        use gitlab::api::{projects::merge_requests::*, *};
        let query = MergeRequestsBuilder::default()
            .project(config.project_id.value())
            .state(MergeRequestState::Opened)
            .build()
            .map_err(|e| anyhow!(e))?;
        paged(query, Pagination::All).query(&gl)?
    };

    info!("Caching the MR info");
    let mr_dir = db_path.join("merge_requests");
    std::fs::create_dir_all(&mr_dir)?;
    for mr in &mrs {
        let path = mr_dir.join(mr.iid.to_string());
        serde_json::to_writer(File::create(path)?, &mr)?;
    }

    info!("Updating the DB with new versions");
    let client = reqwest::blocking::Client::new();
    for mr in &mrs {
        let _s = tracing::info_span!("", mr = %mr.iid).entered();
        if let Err(e) = update_versions(&db, &client, &config, repo, &gl, mr) {
            error!("{e}");
        }
    }

    info!("Checking in on open MRs we didn't get an update for");
    let mrs: HashSet<MergeRequestInternalId> = mrs.into_iter().map(|mr| mr.iid).collect();
    for entry in std::fs::read_dir(mr_dir)? {
        let entry = entry?;
        let id = MergeRequestInternalId::new(entry.file_name().into_string().unwrap().parse()?);
        if mrs.contains(&id) {
            // We already saw this one, it's still open
            continue;
        }
        let mr: MergeRequest = serde_json::from_reader(File::open(entry.path())?)?;
        if mr.state != MergeRequestState::Opened {
            // This MR is closed, that's why we didn't see it in the results
            continue;
        }
        info!("What has happened to !{}..?", mr.iid.value());
        let q = {
            use gitlab::api::projects::merge_requests::*;
            MergeRequestBuilder::default()
                .project(config.project_id.value())
                .merge_request(mr.id.value())
                .build()?
        };
        use gitlab::api::Query;
        let new_info: MergeRequest = match q.query(&gl) {
            Ok(x) => x,
            Err(gitlab::api::ApiError::Gitlab { msg }) if msg == "404 Not found" => {
                let path = entry.path();
                warn!("MR is gone! Deleting {}...", path.display());
                std::fs::remove_file(path)?;
                continue;
            }
            Err(e) => {
                error!("{}: {}", mr.iid.value(), e);
                continue;
            }
        };
        serde_json::to_writer(File::create(entry.path())?, &new_info)?;
        println!(
            "Status of !{} changed to {}",
            mr.iid,
            crate::fmt_state(new_info.state)
        );
        if let Err(e) = update_versions(&db, &client, &config, repo, &gl, &new_info) {
            error!("{e}");
        }
    }

    Ok(())
}

fn update_versions(
    db: &crate::mr_db::Db,
    client: &reqwest::blocking::Client,
    config: &GitlabConfig,
    repo: &Repository,
    gl: &Gitlab,
    mr: &MergeRequest,
) -> anyhow::Result<()> {
    let mr_iid = mr.iid.value();
    let latest = db.get_versions(mr.iid).last().transpose()?;
    // We only update the DB if the head has changed.  Technically we
    // should re-check the base each time as well (in case the target
    // branch has changed); however, this means making an API request
    // per-MR, and is slow.
    let current_head = Oid::from_str(mr.sha.as_ref().unwrap().value())?;
    if latest.map(|x| x.head) == Some(current_head) {
        info!("Skipping MR since its head rev hasn't changed");
        return Ok(());
    }
    let recent_versions = match query_versions(client, config, mr.iid, db) {
        Ok(x) => x,
        Err(e) => {
            error!("Couldn't query the version history: {e}");
            info!("Falling back to recording the current state as the lastest version");
            let info = VersionInfo {
                version: latest.map_or(Version(0), |x| Version(x.version.0 + 1)),
                base: mr_base(repo, gl, config.project_id, mr, current_head)?,
                head: current_head,
            };
            vec![info]
        }
    };
    for &info in &recent_versions {
        let prev = db.insert_version(mr.iid, info)?;
        if let Some(prev) = prev {
            if prev != info {
                warn!("Changed existing version! Was {prev}, now {info}");
            }
        } else {
            let ref_name = format!("refs/orpa/{}_{}/{}", mr_iid, mr.source_branch, info.version);
            let reflog_msg = format!("orpa: creating ref for !{} {}", mr_iid, info.version);
            match repo.reference(&ref_name, info.head, false, &reflog_msg) {
                Ok(_) => info!("Created ref {ref_name}"),
                Err(e) => error!("Couldn't create ref {ref_name}: {e}"),
            }
            println!("Inserted {info}");
        }
    }
    if let Some(info) = recent_versions.last() {
        println!("Updated !{mr_iid} to {}", info.version);
    }
    Ok(())
}

fn mr_base<'a>(
    repo: &'a Repository,
    gl: &'a Gitlab,
    project_id: ProjectId,
    mr: &'a MergeRequest,
    head: Oid,
) -> anyhow::Result<Oid> {
    if let Some(x) = mr.diff_refs.as_ref().and_then(|x| x.base_sha.as_ref()) {
        // They told us the base; good - use that.
        Ok(Oid::from_str(x.value())?)
    } else {
        // Looks like we're gonna have to work it out ourselves...
        use gitlab::api::{projects::repository::branches::Branch, Query};
        // Get the target SHA directly from gitlab, in case the local repo
        // is out-of-date.
        let branch: gitlab::RepoBranch = Branch::builder()
            .project(project_id.value())
            .branch(&mr.target_branch)
            .build()
            .map_err(anyhow::Error::msg)?
            .query(gl)?;
        let target = Oid::from_str(branch.commit.unwrap().id.value())?;
        Ok(repo.merge_base(head, target)?)
    }
}

/// Get the version history from gitlab.  If this endpoint is available,
/// it's the best thing to use.
///
/// Note that gitlab only tells us the 20 most recent versions.
fn query_versions(
    client: &reqwest::blocking::Client,
    config: &GitlabConfig,
    mr_iid: MergeRequestInternalId,
    db: &crate::mr_db::Db,
) -> anyhow::Result<Vec<VersionInfo>> {
    info!("Querying for versions");
    let resp: Vec<serde_json::Value> = client
        .get(format!(
            "https://{}/api/v4/projects/{}/merge_requests/{}/versions",
            config.host,
            config.project_id,
            mr_iid.value(),
        ))
        .header("PRIVATE-TOKEN", &config.token)
        .send()?
        .json()?;

    fn json_to_base(x: &serde_json::Value) -> anyhow::Result<Oid> {
        x["base_commit_sha"]
            .as_str()
            .ok_or_else(|| anyhow!("Bad string"))
            .and_then(|x| Ok(Oid::from_str(x)?))
    }
    fn json_to_head(x: &serde_json::Value) -> anyhow::Result<Oid> {
        x["head_commit_sha"]
            .as_str()
            .ok_or_else(|| anyhow!("Bad string"))
            .and_then(|x| Ok(Oid::from_str(x)?))
    }

    let start_at = match resp.first() {
        Some(first) => {
            let base = json_to_base(first)?;
            let head = json_to_head(first)?;
            db.get_versions(mr_iid)
                .rev()
                .filter_map(|x| x.ok())
                .find(|x| x.head == head && x.base == base)
                .map(|x| x.version)
                .or_else(|| {
                    let latest = db.latest_version(mr_iid).ok()??;
                    Some(Version(latest.version.0 + 1))
                })
                .unwrap_or(Version(0))
        }
        None => return Ok(vec![]),
    };
    resp.into_iter()
        .rev()
        .enumerate()
        .map(|(i, x)| {
            Ok(VersionInfo {
                version: Version(start_at.0 + i as u8),
                base: json_to_base(&x)?,
                head: json_to_head(&x)?,
            })
        })
        .collect()
}
