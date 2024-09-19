use crate::mr_db::VersionInfo;
use crate::{get_idx, OPTS};
use anyhow::anyhow;
use chrono::{DateTime, NaiveDateTime};
use enum_map::{Enum, EnumMap};
use git2::{Commit, Diff, DiffStatsFormat, ErrorCode, Oid, Repository, Time, Tree};
use itertools::Itertools;
use sha1::{Digest, Sha1};
use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::io::Write;
use std::path::Path;
use std::sync::{LazyLock, OnceLock};
use tracing::*;
use yansi::Paint;

pub fn append_note(repo: &Repository, oid: Oid, new_note: &str) -> anyhow::Result<()> {
    let sig = repo.signature()?;
    let old_note = get_note(repo, oid)?;
    let mut notes = HashSet::new();
    if let Some(note) = old_note.as_ref() {
        for line in note.lines() {
            notes.insert(line);
        }
    }
    notes.insert(new_note);
    let combined_note = notes.iter().join("\n");
    let notes_ref = notes_ref();
    repo.note(&sig, &sig, notes_ref, oid, &combined_note, true)?;
    println!("{}: {}", oid, notes.iter().join(", "));
    Ok(())
}

fn notes_ref() -> Option<&'static str> {
    static NOTES_REF: LazyLock<Option<String>> =
        LazyLock::new(|| OPTS.notes_ref.as_ref().map(|x| format!("refs/notes/{}", x)));
    NOTES_REF.as_ref().map(|x| x.as_str())
}

pub fn get_note(repo: &Repository, oid: Oid) -> anyhow::Result<Option<String>> {
    let notes_ref = notes_ref();
    match repo.find_note(notes_ref, oid) {
        Ok(note) => Ok(note.message().map(|x| x.to_owned())),
        Err(e) if e.code() == ErrorCode::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Actually returns all notes...
pub fn recent_notes(repo: &Repository) -> anyhow::Result<Vec<Oid>> {
    let notes_ref = notes_ref().unwrap_or("refs/notes/commits");
    let notes = match repo.find_reference(notes_ref) {
        Ok(x) => x,
        Err(_) => return Ok(vec![]),
    };
    let tree = notes.peel_to_commit()?.tree()?;
    let mut ret = Vec::with_capacity(tree.len());
    for x in tree.iter() {
        let name = x
            .name()
            .ok_or_else(|| anyhow!("Commit is not even unicode, let alone hex!"))?;
        ret.push(Oid::from_str(name)?);
    }
    Ok(ret)
}

/// Iterate over the lines in the commit's textual representation.
///
/// Covers the commit message and diff, but no other metadata.
macro_rules! commit_lines {
    ($repo:expr, $commit: expr) => {
        String::from_utf8_lossy(
            &git2::Email::from_diff(
                &commit_diff($repo, $commit)?,
                1,
                1,
                &$commit.id(),
                "",
                "",
                &git2::Signature::now("", "")?,
                &mut git2::EmailCreateOptions::new(),
            )?
            .as_slice(),
        )
        .lines()
        // Drop the OID, author, and date
        .skip(3)
    };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Comparison {
    // Total number of unique lines in the left
    pub lines_in_left: usize,
    // Number of unique lines in both left and right
    pub lines_in_both: usize,
    // Total number of unique lines in the right
    pub lines_in_right: usize,
}

impl Comparison {
    pub fn score(self) -> f64 {
        2. * self.lines_in_both as f64 / (self.lines_in_left as f64 + self.lines_in_right as f64)
    }
}

/// For each reviewed commit, compute its similarity to the given commit.
///
/// Simliarity is defined as follows:
///
/// > number of distinct lines in common / number of distinct lines
///
/// Note that this means that a commit which is a superset will get a
/// perfect score.
pub fn similiar_commits(repo: &Repository, c: &Commit) -> anyhow::Result<Vec<(Oid, Comparison)>> {
    let idx = get_idx(repo)?;
    let mut scores: HashMap<Oid, usize> = HashMap::new();
    let all_lines: HashSet<Line> = commit_lines!(repo, c)
        .map(|line| Line(Sha1::digest(line).into()))
        .collect();
    for &digest in &all_lines {
        for oid in idx.commits_containing(digest)? {
            *(scores.entry(oid).or_default()) += 1;
        }
    }
    let lines_in_left = all_lines.len();
    let mut scores = scores
        .into_iter()
        .map(|(oid, lines_in_both)| {
            let lines_in_right = idx.lines_in(&oid).unwrap().len();
            assert!(lines_in_both <= lines_in_left);
            assert!(lines_in_both <= lines_in_right);
            (
                oid,
                Comparison {
                    lines_in_left,
                    lines_in_both,
                    lines_in_right,
                },
            )
        })
        .collect::<Vec<_>>();
    scores.sort_by(|(_, x), (_, y)| x.score().partial_cmp(&y.score()).unwrap().reverse());
    Ok(scores)
}

pub struct LineIdx {
    /// What lines does this commit contain? (Oid => [Line])
    pub forward: sled::Tree,
    /// In what commits does this line appear? (Line => [Oid])
    pub reverse: sled::Tree,
}

/// The SHA1 of a line in a commit's textual representation.
#[derive(PartialEq, Eq, Copy, Clone, Hash)]
pub struct Line(pub [u8; 20]);

impl LineIdx {
    pub fn commits_containing(&self, line: Line) -> anyhow::Result<Vec<Oid>> {
        let bytes = self.reverse.get(line.0)?;
        let bytes = bytes.as_deref().unwrap_or(&[][..]);
        bytes
            .chunks(20)
            .map(|x| Oid::from_bytes(x).map_err(|e| e.into()))
            .collect()
    }

    pub fn lines_in(&self, oid: &Oid) -> anyhow::Result<Vec<Line>> {
        let bytes = self.forward.get(oid.as_bytes())?;
        let bytes = bytes.as_deref().unwrap_or(&[][..]);
        bytes.chunks(20).map(|x| Ok(Line(x.try_into()?))).collect()
    }

    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let db = sled::open(path)?;
        let forward = db.open_tree("forward")?;
        let reverse = db.open_tree("reverse")?;
        fn append(_: &[u8], existing: Option<&[u8]>, incoming: &[u8]) -> Option<Vec<u8>> {
            let mut ret = existing.unwrap_or_default().to_vec();
            ret.extend_from_slice(incoming);
            Some(ret)
        }
        reverse.set_merge_operator(append);
        Ok(LineIdx { forward, reverse })
    }

    // TODO: (perf) Drop very popular lines (eg. "" and "---")
    pub fn refresh(&self, repo: &Repository) -> anyhow::Result<()> {
        let time = std::time::Instant::now();
        for oid in recent_notes(repo)? {
            if self.forward.get(oid.as_bytes())?.is_some() {
                continue;
            }
            let commit = repo.find_commit(oid)?;
            let all_lines = commit_lines!(repo, &commit)
                .map(|line| Line(Sha1::digest(line).into()))
                .collect::<HashSet<_>>();
            let mut all_lines_b = vec![];
            for digest in &all_lines {
                self.reverse.merge(digest.0, oid)?;
                all_lines_b.extend_from_slice(&digest.0);
            }
            self.forward.insert(oid, all_lines_b)?;
        }
        tracing::info!("Refreshed the index in {:?}", time.elapsed());
        Ok(())
    }
}

// TODO: Include addresses from the mailmap
fn our_email(repo: &Repository) -> &'static [u8] {
    static SIG: OnceLock<Vec<u8>> = OnceLock::new();
    SIG.get_or_init(|| {
        let sig = repo.signature().unwrap();
        sig.email_bytes().to_vec()
    })
    .as_slice()
}

fn reviewed_commits(repo: &Repository) -> &'static HashMap<Oid, bool> {
    static REVIEWS: OnceLock<HashMap<Oid, bool>> = OnceLock::new();
    REVIEWS.get_or_init(|| {
        let f = || {
            let mut wtr = repo.blob_writer(None)?;
            wtr.write_all(b"checkpoint")?;
            let checkpoint_oid = wtr.commit()?;
            info!("Checkpoint OID is {}", checkpoint_oid);

            let mut reviews = HashMap::new();
            for x in repo.notes(notes_ref())? {
                let (note_oid, commit_oid) = x?;
                reviews.insert(commit_oid, note_oid == checkpoint_oid);
            }
            info!("Scanned {} reviews", reviews.len());
            anyhow::Ok(reviews)
        };
        f().unwrap()
    })
}

pub fn lookup(repo: &Repository, oid: Oid) -> anyhow::Result<Status> {
    match reviewed_commits(repo).get(&oid) {
        Some(true) => Ok(Status::Checkpoint),
        Some(false) => Ok(Status::Reviewed),
        None => {
            let commit = repo.find_commit(oid)?;
            if commit.author().email_bytes() == our_email(repo) {
                Ok(Status::Ours)
            } else if commit.parent_count() > 1 {
                Ok(Status::Merge)
            } else {
                let mut reviewed = false;
                if OPTS.dedup {
                    let digest = commit_diff_digest(repo, &commit)?;
                    for (other_oid, _) in similiar_commits(repo, &commit)?
                        .into_iter()
                        .filter(|(_, ddiff)| ddiff.score() == 1.)
                    {
                        let other = repo.find_commit(other_oid)?;
                        let other_digest = commit_diff_digest(repo, &other)?;
                        if digest == other_digest {
                            reviewed = true;
                            break;
                        }
                    }
                }
                if reviewed {
                    tracing::info!("Found a commit that matches!");
                    // TODO: Copy over the note
                    Ok(Status::Reviewed)
                } else {
                    Ok(Status::New)
                }
            }
        }
    }
}

pub fn walk_new(
    repo: &Repository,
    range: Option<&String>,
    mut f: impl FnMut(Oid),
) -> anyhow::Result<()> {
    let mut walk = repo.revwalk()?;
    if let Some(range) = range {
        walk.push_range(range)?;
    } else {
        walk.push_head()?;
    }
    for oid in walk {
        let oid = oid?;
        let status = lookup(repo, oid)?;
        match status {
            Status::New => f(oid),
            Status::Checkpoint => break,
            _ => (),
        }
    }
    Ok(())
}

pub fn walk_version<'repo>(
    repo: &'repo Repository,
    ver: &VersionInfo,
) -> anyhow::Result<impl Iterator<Item = anyhow::Result<(Oid, Status)>> + 'repo> {
    let mut walk = repo.revwalk()?;
    walk.push_range(&format!("{}..{}", &ver.base.0, &ver.head.0))?;
    Ok(walk
        .map(move |oid| {
            let oid = oid?;
            let status = lookup(repo, oid)?;
            Ok((oid, status))
        })
        .take_while(|x| !matches!(x, Ok((_, Status::Checkpoint)))))
}

pub fn version_stats(
    repo: &Repository,
    ver: &VersionInfo,
) -> anyhow::Result<EnumMap<Status, usize>> {
    let mut stats = EnumMap::default();
    for x in walk_version(repo, ver)? {
        let (_, status) = x?;
        stats[status] += 1;
    }
    Ok(stats)
}

pub fn time_to_chrono(time: Time) -> NaiveDateTime {
    // FIXME: Include timezone
    DateTime::from_timestamp(time.seconds(), 0)
        .unwrap()
        .naive_utc()
}

pub fn show_commit_oneline(repo: &Repository, oid: Oid) -> anyhow::Result<()> {
    let c = repo.find_commit(oid)?;
    println!(
        "  {} {}",
        Paint::yellow(c.as_object().short_id()?.as_str().unwrap_or("")),
        c.summary().unwrap_or(""),
    );
    Ok(())
}

/// The diff of a commit against its first parent
pub fn commit_diff<'a>(repo: &'a Repository, c: &Commit) -> anyhow::Result<Diff<'a>> {
    let base = match c.parent(0) {
        Ok(parent) => parent.tree()?,
        Err(e) if e.code() == ErrorCode::NotFound => empty_tree(repo)?,
        Err(e) => Err(e)?,
    };
    Ok(repo.diff_tree_to_tree(Some(&base), Some(&c.tree()?), None)?)
}

/// The SHA1 of the textual diff of a commit against its first parent
pub fn commit_diff_digest(repo: &Repository, c: &Commit) -> anyhow::Result<Line> {
    let diff = commit_lines!(repo, c).join("\n");
    Ok(Line(Sha1::digest(diff).into()))
}

pub fn empty_tree(repo: &Repository) -> anyhow::Result<Tree> {
    let oid = repo.treebuilder(None)?.write()?;
    Ok(repo.find_tree(oid)?)
}

pub fn show_commit_with_diffstat(repo: &Repository, oid: Oid) -> anyhow::Result<()> {
    let c = repo.find_commit(oid)?;
    println!(
        "{}{}",
        Paint::yellow("commit "),
        Paint::yellow(oid.to_string())
    );
    println!(
        "Author: {} <{}>",
        c.author().name().unwrap_or(""),
        c.author().email().unwrap_or("")
    );
    println!("Date:   {}", time_to_chrono(c.author().when()));
    println!();
    for line in c.message().into_iter().flat_map(|x| x.lines()) {
        println!("    {}", line);
    }
    println!();
    // FIXME: Stats are wrong for merge commits
    let diff = commit_diff(repo, &c)?;
    let stats = diff.stats()?.to_buf(DiffStatsFormat::FULL, 80)?;
    print!("{}", stats.as_str().unwrap_or(""));
    Ok(())
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Enum)]
pub enum Status {
    Reviewed,
    Checkpoint,
    Ours,
    Merge,
    New,
}
