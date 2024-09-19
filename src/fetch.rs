use crate::{db_path, mr_db::MRWithVersions, GitlabConfig, Version, VersionInfo};
use anyhow::anyhow;
use chrono::{DateTime, Utc};
use git2::{Oid, Repository};
use gitlab::Gitlab;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use tracing::*;

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
pub struct MergeRequestId(pub u64);

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
pub struct MergeRequestInternalId(pub u64);

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub struct ProjectId(pub u64);

#[derive(Serialize, Deserialize, Debug, Clone, Hash, PartialEq, Eq)]
pub struct ObjectId(pub String);

impl From<Oid> for ObjectId {
    fn from(oid: Oid) -> Self {
        ObjectId(oid.to_string())
    }
}

impl ObjectId {
    pub fn as_oid(&self) -> Oid {
        Oid::from_str(&self.0).unwrap()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeRequestState {
    #[serde(rename = "opened")]
    Opened,
    #[serde(rename = "closed")]
    Closed,
    #[serde(rename = "reopened")]
    Reopened,
    #[serde(rename = "merged")]
    Merged,
    #[serde(rename = "locked")]
    Locked,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MergeRequest {
    pub id: MergeRequestId,
    pub iid: MergeRequestInternalId,
    pub project_id: ProjectId,
    pub title: String,
    pub description: Option<String>,
    pub draft: bool,
    pub state: MergeRequestState,
    pub updated_at: DateTime<Utc>,
    pub target_branch: String,
    pub source_branch: String,
    pub author: UserBasic,
    pub assignee: Option<UserBasic>,
    pub assignees: Option<Vec<UserBasic>>,
    pub reviewers: Option<Vec<UserBasic>>,
    pub sha: Option<ObjectId>,
    pub diff_refs: Option<DiffRefs>,
    // Also: created_at, merged_at, closed_at, merged_by, closed_by,
    // upvotes, downvotes, source_project_id, target_project_id,
    // labels, allow_collaboration, allow_maintainer_to_push, milestone,
    // squash, merge_when_pipeline_succeeds, merge_status, merge_error,
    // rebase_in_progress, merge_commit_sha, squash_commit_sha, subscribed,
    // time_stats, blocking_discussions_resolved, changes_count,
    // user_notes_count, discussion_locked, should_remove_source_branch,
    // force_remove_source_branch, has_conflicts, user, web_url, pipeline,
    // first_contribution
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UserBasic {
    pub username: String,
    pub name: String,
    // Also: id, state, avatar_url, web_url
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct DiffRefs {
    pub base_sha: Option<ObjectId>,
    // Also: head_sha, start_sha
}

pub fn fetch(repo: &Repository) -> anyhow::Result<()> {
    let config = GitlabConfig::load(repo)?;

    let db_path = db_path(repo);
    let mr_dir = db_path.join("merge_requests");

    info!("Connecting to gitlab at {}", config.host);
    let gl = Gitlab::new(&config.host, &config.token)?;

    println!("Fetching open MRs for project {}...", config.project_id.0);
    let mrs: Vec<MergeRequest> = {
        use gitlab::api::{projects::merge_requests::*, *};
        let query = MergeRequestsBuilder::default()
            .project(config.project_id.0)
            .state(MergeRequestState::Opened)
            .build()
            .map_err(|e| anyhow!(e))?;
        paged(query, Pagination::All).query(&gl)?
    };

    info!("Updating the DB with new versions");
    std::fs::create_dir_all(&mr_dir)?;
    let client = reqwest::blocking::Client::new();
    for mr in &mrs {
        let _s = tracing::info_span!("", mr = mr.iid.0).entered();
        let path = mr_dir.join(mr.iid.0.to_string());
        let mut versions = match std::fs::read_to_string(&path) {
            Ok(txt) => serde_json::from_str::<MRWithVersions>(&txt)?.versions,
            Err(_) => BTreeMap::default(),
        };
        if let Err(e) = update_versions(mr, &mut versions, &client, &config, repo, &gl) {
            error!("{e}");
        }

        serde_json::to_writer(
            File::create(path)?,
            &MRWithVersions {
                mr: mr.clone(),
                versions,
            },
        )?;
    }

    info!("Checking in on open MRs we didn't get an update for");
    let mrs: HashSet<MergeRequestInternalId> = mrs.into_iter().map(|mr| mr.iid).collect();
    for entry in std::fs::read_dir(mr_dir)? {
        let entry = entry?;
        let id = MergeRequestInternalId(entry.file_name().into_string().unwrap().parse()?);
        if mrs.contains(&id) {
            // We already saw this one, it's still open
            continue;
        }
        let MRWithVersions { mr, mut versions } =
            serde_json::from_reader(File::open(entry.path())?)?;
        if mr.state != MergeRequestState::Opened {
            // This MR is closed, that's why we didn't see it in the results
            continue;
        }

        info!("What has happened to !{}..?", mr.iid.0);
        let q = {
            use gitlab::api::projects::merge_requests::*;
            MergeRequestBuilder::default()
                .project(config.project_id.0)
                .merge_request(mr.id.0)
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
                error!("{}: {}", mr.iid.0, e);
                continue;
            }
        };
        println!(
            "Status of !{} changed to {}",
            mr.iid.0,
            crate::fmt_state(new_info.state)
        );
        if let Err(e) = update_versions(&new_info, &mut versions, &client, &config, repo, &gl) {
            error!("{e}");
        }
        serde_json::to_writer(
            File::create(entry.path())?,
            &MRWithVersions {
                mr: new_info,
                versions,
            },
        )?;
    }

    Ok(())
}

fn update_versions(
    mr: &MergeRequest,
    versions: &mut BTreeMap<Version, VersionInfo>,
    client: &reqwest::blocking::Client,
    config: &GitlabConfig,
    repo: &Repository,
    gl: &Gitlab,
) -> anyhow::Result<()> {
    let mr_iid = mr.iid.0;
    let latest = versions.last_key_value();
    // We only update the DB if the head has changed.  Technically we
    // should re-check the base each time as well (in case the target
    // branch has changed); however, this means making an API request
    // per-MR, and is slow.
    let current_head = mr.sha.as_ref().unwrap();
    if latest.as_ref().map(|x| &x.1.head) == Some(current_head) {
        info!("Skipping MR since its head rev hasn't changed");
        return Ok(());
    }
    let recent_versions = match query_versions(client, config, mr.iid, versions) {
        Ok(x) => x,
        Err(e) => {
            error!("Couldn't query the version history: {e}");
            info!("Falling back to recording the current state as the lastest version");
            let version = latest.map_or(Version(0), |x| Version(x.0 .0 + 1));
            let info = VersionInfo {
                base: mr_base(repo, gl, config.project_id, mr, current_head.as_oid())?,
                head: current_head.clone(),
            };
            vec![(version, info)]
        }
    };
    for (version, info) in &recent_versions {
        let prev = versions.insert(*version, info.clone());
        if let Some(prev) = &prev {
            if prev != info {
                warn!("Changed existing version! Was {prev}, now {info}");
            }
        } else {
            let ref_name = format!("refs/orpa/{}_{}/{}", mr_iid, mr.source_branch, version);
            let reflog_msg = format!("orpa: creating ref for !{} {}", mr_iid, version);
            match repo.reference(&ref_name, info.head.as_oid(), false, &reflog_msg) {
                Ok(_) => info!("Created ref {ref_name}"),
                Err(e) => error!("Couldn't create ref {ref_name}: {e}"),
            }
            println!("Inserted {info}");
        }
    }
    if let Some((version, _)) = recent_versions.last() {
        println!("Updated !{mr_iid} to {}", version);
    }
    Ok(())
}

fn mr_base<'a>(
    repo: &'a Repository,
    gl: &'a Gitlab,
    project_id: ProjectId,
    mr: &'a MergeRequest,
    head: Oid,
) -> anyhow::Result<ObjectId> {
    if let Some(x) = mr.diff_refs.as_ref().and_then(|x| x.base_sha.clone()) {
        // They told us the base; good - use that.
        Ok(x)
    } else {
        // Looks like we're gonna have to work it out ourselves...
        use gitlab::api::{projects::repository::branches::Branch, Query};

        #[derive(Serialize, Deserialize)]
        struct RepoBranch {
            commit: Option<RepoCommit>,
            // Also: name, merged, protected, developers_can_{push,merge},
            // can_push, default
        }
        #[derive(Serialize, Deserialize)]
        struct RepoCommit {
            id: ObjectId,
            // Also: short_id, title, parent_ids, {author,committer}_{name,email},
            // {authored,committed}_date, created_at, message
        }

        // Get the target SHA directly from gitlab, in case the local repo
        // is out-of-date.
        let branch: RepoBranch = Branch::builder()
            .project(project_id.0)
            .branch(&mr.target_branch)
            .build()
            .map_err(anyhow::Error::msg)?
            .query(gl)?;
        let target = branch.commit.unwrap().id.as_oid();
        let base = repo.merge_base(head, target)?;
        Ok(base.into())
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
    versions: &BTreeMap<Version, VersionInfo>,
) -> anyhow::Result<Vec<(Version, VersionInfo)>> {
    info!("Querying for versions");
    let resp: Vec<serde_json::Value> = client
        .get(format!(
            "https://{}/api/v4/projects/{}/merge_requests/{}/versions",
            config.host, config.project_id.0, mr_iid.0,
        ))
        .header("PRIVATE-TOKEN", &config.token)
        .send()?
        .json()?;

    fn json_to_base(x: &serde_json::Value) -> anyhow::Result<ObjectId> {
        x["base_commit_sha"]
            .as_str()
            .ok_or_else(|| anyhow!("Bad string"))
            .map(|x| ObjectId(x.to_owned()))
    }
    fn json_to_head(x: &serde_json::Value) -> anyhow::Result<ObjectId> {
        x["head_commit_sha"]
            .as_str()
            .ok_or_else(|| anyhow!("Bad string"))
            .map(|x| ObjectId(x.to_owned()))
    }

    let start_at = match resp.first() {
        Some(first) => {
            let base = json_to_base(first)?;
            let head = json_to_head(first)?;
            versions
                .iter()
                .rev()
                .find(|(_, x)| x.head == head && x.base == base)
                .map(|(x, _)| *x)
                .or_else(|| {
                    let (latest, _) = versions.last_key_value()?;
                    Some(Version(latest.0 + 1))
                })
                .unwrap_or(Version(0))
        }
        None => return Ok(vec![]),
    };
    resp.into_iter()
        .rev()
        .enumerate()
        .map(|(i, x)| {
            let version = Version(start_at.0 + i as u8);
            let info = VersionInfo {
                base: json_to_base(&x)?,
                head: json_to_head(&x)?,
            };
            Ok((version, info))
        })
        .collect()
}
