use git2::Oid;
use gitlab::MergeRequestInternalId;
use std::fmt;
use std::path::Path;

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
    // TODO: pub time: DateTime,
    pub base: Oid,
    pub head: Oid,
}

impl fmt::Display for VersionInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}: {}..{}", self.version, self.base, self.head)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version(pub u8);

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "v{}", self.0 + 1)
    }
}

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        Ok(Db(sled::open(path)?))
    }

    pub fn latest_version(
        &self,
        mr_id: MergeRequestInternalId,
    ) -> anyhow::Result<Option<VersionInfo>> {
        let mr_id = mr_id.value().to_le_bytes();
        let existing = self.0.scan_prefix(mr_id);
        let Some(x) = existing.last() else {
            return Ok(None);
        };
        let (k, v) = x?;
        let version = Version(k[8]);
        let base = Oid::from_bytes(&v[..20])?;
        let head = Oid::from_bytes(&v[20..])?;
        Ok(Some(VersionInfo {
            version,
            base,
            head,
        }))
    }

    pub fn get_versions(
        &self,
        mr_id: MergeRequestInternalId,
    ) -> impl DoubleEndedIterator<Item = anyhow::Result<VersionInfo>> {
        let mr_id = mr_id.value().to_le_bytes();
        let existing = self.0.scan_prefix(mr_id);
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

    pub fn insert_version(
        &self,
        mr_id: MergeRequestInternalId,
        info: VersionInfo,
    ) -> anyhow::Result<Option<VersionInfo>> {
        let mut key = [0; 9];
        key[..8].copy_from_slice(&mr_id.value().to_le_bytes());
        key[8] = info.version.0;
        let mut val = Box::new([0; 40]);
        val[..20].copy_from_slice(info.base.as_bytes());
        val[20..].copy_from_slice(info.head.as_bytes());
        self.0
            .insert(key, val as Box<[u8]>)?
            .map(|xs| {
                anyhow::Ok(VersionInfo {
                    version: info.version,
                    base: Oid::from_bytes(&xs[..20])?,
                    head: Oid::from_bytes(&xs[20..])?,
                })
            })
            .transpose()
    }
}
