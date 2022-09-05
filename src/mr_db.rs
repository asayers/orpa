use git2::{Oid, Repository};
use gitlab::{Gitlab, MergeRequest, ProjectId};
use std::fmt;
use std::path::Path;
use tracing::*;

/// A database which stores MR versions.
///
/// # Database schema
///
/// Logically, the DB is a map from (merge request ID, version number) =>
/// (base OID, head OID).
///
/// Keys: the MR ID (8 bytes) followed by the version number (1 byte).
/// Values: the base OID (20 bytes) followed by the head OID (20 bytes).
pub struct Db(sled::Db);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VersionInfo {
    pub version: Version,
    pub base: Oid,
    pub head: Oid,
}

impl fmt::Display for VersionInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}: {}..{}", self.version, self.base, self.head)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version(u8);

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "v{}", self.0 + 1)
    }
}

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        Ok(Db(sled::open(path)?))
    }

    pub fn get_versions(
        &self,
        mr: &MergeRequest,
    ) -> impl Iterator<Item = anyhow::Result<VersionInfo>> {
        let mr_id = mr.iid.value().to_le_bytes();
        let existing = self.0.scan_prefix(&mr_id);
        existing.map(|x| {
            let (k, v) = x?;
            let version = Version(k[8]);
            let base = Oid::from_bytes(&v[..20])?;
            let head = Oid::from_bytes(&v[20..])?;
            Ok(VersionInfo {
                version,
                base,
                head,
            })
        })
    }

    fn insert_version(&self, mr: &MergeRequest, info: VersionInfo) -> anyhow::Result<()> {
        let mut key = [0; 9];
        let mr_id = mr.iid.value().to_le_bytes();
        key[..8].copy_from_slice(&mr_id);
        key[8] = info.version.0;
        let mut val = Box::new([0; 40]);
        val[..20].copy_from_slice(info.base.as_bytes());
        val[20..].copy_from_slice(info.head.as_bytes());
        self.0.insert(key, val as Box<[u8]>)?;
        Ok(())
    }

    pub fn insert_if_newer(
        &self,
        repo: &Repository,
        gl: &Gitlab,
        project_id: ProjectId,
        mr: &MergeRequest,
    ) -> anyhow::Result<Option<VersionInfo>> {
        let latest = self.get_versions(mr).last().transpose()?;
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
            self.insert_version(mr, info)?;
            Ok(Some(info))
        } else {
            Ok(None)
        }
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
