use chrono::NaiveDateTime;
use git2::{Commit, Diff, DiffStatsFormat, ErrorCode, Oid, Repository, Time, Tree};
use itertools::Itertools;
use std::collections::HashSet;
use yansi::Paint;

const NOTES_REF: &str = "refs/notes/orpa";

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
    repo.note(&sig, &sig, Some(NOTES_REF), oid, &combined_note, true)?;
    println!("{}: {}", oid, notes.iter().join(", "));
    Ok(())
}

pub fn get_note(repo: &Repository, oid: Oid) -> anyhow::Result<Option<String>> {
    match repo.find_note(Some(NOTES_REF), oid) {
        Ok(note) => Ok(note.message().map(|x| x.to_owned())),
        Err(e) if e.code() == ErrorCode::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn lookup(repo: &Repository, oid: Oid) -> anyhow::Result<Status> {
    match get_note(repo, oid)? {
        Some(note) if note.lines().any(|x| x == "checkpoint") => Ok(Status::Checkpoint),
        Some(_) => Ok(Status::Reviewed),
        None => {
            let commit = repo.find_commit(oid)?;
            let sig = repo.signature()?;
            if commit.author().name_bytes() == sig.name_bytes() {
                Ok(Status::Ours)
            } else if commit.parent_count() > 1 {
                Ok(Status::Merge)
            } else {
                Ok(Status::New)
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
        let status = lookup(&repo, oid)?;
        match status {
            Status::New => f(oid),
            Status::Checkpoint => break,
            _ => (),
        }
    }
    Ok(())
}

pub fn time_to_chrono(time: Time) -> chrono::NaiveDateTime {
    // FIXME: Include timezone
    NaiveDateTime::from_timestamp(time.seconds(), 0)
}

pub fn show_commit_oneline(repo: &Repository, oid: Oid) -> anyhow::Result<()> {
    let c = repo.find_commit(oid)?;
    // FIXME: Stats are wrong for merge commits
    let diff = commit_diff(repo, &c)?;
    let stats = diff.stats()?.to_buf(DiffStatsFormat::SHORT, 20)?;
    println!(
        "{} {:<80} {}",
        Paint::yellow(c.as_object().short_id()?.as_str().unwrap_or("")),
        c.summary().unwrap_or(""),
        Paint::blue(stats.as_str().unwrap_or("").trim()),
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

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum Status {
    Reviewed,
    Checkpoint,
    Ours,
    Merge,
    New,
}
