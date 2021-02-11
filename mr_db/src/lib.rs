use git2::Oid;
use gitlab::MergeRequest;
use sled::Db;

// # Database schema
//
// Logically, the DB is a map from (merge request ID, revision number) =>
// (base OID, head OID).
//
// Keys: the MR ID (8 bytes) followed by the revision number (1 byte).
// Values: the base OID (20 bytes) followed by the head OID (20 bytes).

#[derive(Clone, Copy, Debug)]
pub struct RevInfo {
    pub rev: u8,
    pub base: Oid,
    pub head: Oid,
}

pub fn get_revs(db: &Db, mr: &MergeRequest) -> impl Iterator<Item = anyhow::Result<RevInfo>> {
    let mr_id = mr.iid.value().to_le_bytes();
    let existing = db.scan_prefix(&mr_id);
    existing.map(|x| {
        let (k, v) = x?;
        let rev: u8 = k[8];
        let base = Oid::from_bytes(&v[..20])?;
        let head = Oid::from_bytes(&v[20..])?;
        Ok(RevInfo { rev, base, head })
    })
}

pub fn insert_rev(db: &Db, mr: &MergeRequest, info: RevInfo) -> anyhow::Result<()> {
    let mut key = [0; 9];
    let mr_id = mr.iid.value().to_le_bytes();
    key[..8].copy_from_slice(&mr_id);
    key[8] = info.rev;
    let mut val = Box::new([0; 40]);
    val[..20].copy_from_slice(info.base.as_bytes());
    val[20..].copy_from_slice(info.head.as_bytes());
    db.insert(key, val as Box<[u8]>)?;
    Ok(())
}
