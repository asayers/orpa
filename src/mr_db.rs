use crate::fetch::{MergeRequest, ObjectId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MRWithVersions {
    #[serde(flatten)]
    pub mr: MergeRequest,
    #[serde(default)]
    pub versions: BTreeMap<Version, VersionInfo>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: Version,
    // TODO: pub time: DateTime,
    pub base: ObjectId,
    pub head: ObjectId,
}

impl fmt::Display for VersionInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}: {}..{}", self.version, self.base.0, self.head.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
pub struct Version(pub u8);

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "v{}", self.0 + 1)
    }
}
