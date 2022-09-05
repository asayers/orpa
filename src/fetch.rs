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
    for mr in &mrs {
        match insert_if_newer(&db, &repo, &gl, config.project_id, mr) {
            Ok(Some(info)) => println!("Updated !{} to {}", mr.iid.value(), info.version),
            Ok(None) => (),
            Err(e) => error!("{}", e),
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
        if let Some(info) = insert_if_newer(&db, &repo, &gl, config.project_id, &new_info)? {
            println!("Updated !{} to {}", mr.iid.value(), info.version);
        }
        println!(
            "Status of !{} changed to {}",
            mr.iid,
            crate::fmt_state(new_info.state)
        );
    }

    Ok(())
}

fn insert_if_newer(
    db: &crate::mr_db::Db,
    repo: &Repository,
    gl: &Gitlab,
    project_id: ProjectId,
    mr: &MergeRequest,
) -> anyhow::Result<Option<VersionInfo>> {
    let latest = db.get_versions(mr).last().transpose()?;
    // We only update the DB if the head has changed.  Technically we
    // should re-check the base each time as well (in case the target
    // branch has changed); however, this means making an API request
    // per-MR, and is slow.
    let current_head = Oid::from_str(mr.sha.as_ref().unwrap().value())?;
    if latest.map(|x| x.head) != Some(current_head) {
        let info = VersionInfo {
            version: latest.map_or(Version(0), |x| Version(x.version.0 + 1)),
            base: mr_base(&repo, &gl, project_id, &mr, current_head)?,
            head: current_head,
        };
        info!("Inserting new version: {:?}", info);
        db.insert_version(mr, info)?;
        Ok(Some(info))
    } else {
        Ok(None)
    }
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
