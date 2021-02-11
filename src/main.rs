use anyhow::anyhow;
use chrono::NaiveDateTime;
use colored::Colorize;
use git2::{Commit, Diff, DiffStatsFormat, ErrorCode, Oid, Repository, Time, Tree};
use itertools::Itertools;
use std::collections::HashSet;
use std::io::{stdin, stdout, BufRead, Write};
use std::process::Command;
use structopt::StructOpt;
use tracing::*;

#[derive(StructOpt)]
struct Opts {
    #[structopt(subcommand)]
    cmd: Option<Cmd>,
}
#[derive(StructOpt)]
enum Cmd {
    /// Summarize the review status
    Summary { range: Option<String> },
    /// Interactively review waiting commits
    Triage { range: Option<String> },
    /// Inspect the oldest unreviewed commit
    Next { range: Option<String> },
    /// List all unreviewed commits
    List { range: Option<String> },
    /// Show the status of a commit
    Show { revspec: String },
    /// Attach a note to a commit
    Review {
        revspec: String,
        note: Option<String>,
    },
    /// Approve a commit and all its ancestors
    Checkpoint { revspec: String },
    /// Speed up future operations
    GC,
}

fn main() {
    let opts = Opts::from_args();
    tracing_subscriber::fmt::init();
    match main_2(opts) {
        Ok(()) => (),
        Err(e) => {
            error!("{}", e);
            std::process::exit(1);
        }
    }
}

fn main_2(opts: Opts) -> anyhow::Result<()> {
    let repo = Repository::open_from_env()?;
    match opts.cmd {
        None => summary(&repo, None),
        Some(Cmd::Summary { range }) => summary(&repo, range),
        Some(Cmd::Triage { range }) => triage(&repo, range),
        Some(Cmd::Next { range }) => next(&repo, range),
        Some(Cmd::List { range }) => list(&repo, range),
        Some(Cmd::Show { revspec }) => show(&repo, &revspec),
        Some(Cmd::Review { revspec, note }) => add_note(
            &repo,
            repo.revparse_single(&revspec)?.peel_to_commit()?.id(),
            note.as_ref().map_or("Reviewed", |x| x.as_str()),
        ),
        Some(Cmd::Checkpoint { revspec }) => add_note(
            &repo,
            repo.revparse_single(&revspec)?.peel_to_commit()?.id(),
            "checkpoint",
        ),
        Some(Cmd::GC) => Err(anyhow!("Auto-checkpointing not implemented yet")),
    }
}

fn summary(repo: &Repository, range: Option<String>) -> anyhow::Result<()> {
    let mut new = vec![];
    walk_new(&repo, range.as_ref(), |oid| new.push(oid))?;
    let n_new = new.len();
    if n_new == 0 {
        println!("Everything looks good!");
    } else {
        println!("The following commits are awaiting review:\n");
        for oid in new.into_iter().rev().take(10) {
            show_commit_oneline(&repo, oid)?;
        }
        if n_new > 10 {
            println!("...and {} more (use \"orpa list\" to see them)", n_new - 10);
            println!("\nHint: You have a lot of unreviewed commits. You can skip old\nones by setting a checkpoint:    orpa checkpoint <oid>");
        }
    }
    Ok(())
}

fn triage(repo: &Repository, range: Option<String>) -> anyhow::Result<()> {
    let mut new = vec![];
    walk_new(&repo, range.as_ref(), |oid| new.push(oid))?;
    for oid in new.into_iter().rev() {
        show_commit_with_diffstat(&repo, oid)?;
        println!();
        let new_note = loop {
            print!("> ");
            stdout().flush()?;
            let mut l = String::new();
            stdin().lock().read_line(&mut l)?;
            match l.trim() {
                _ if l.is_empty() => return Ok(()), // ctrl-D
                "q" | "quit" => return Ok(()),
                "h" | "help" => println!("mark      leave a note\nskip      review again next time\ntig       open in tig\nquit      end review session"),
                "next"|"skip" => break None,
                "mark" => break Some("Reviewed".into()),
                x if x.starts_with("mark ") => break Some(String::from(&x[5..])),
                "tig" => {
                    let status = Command::new("tig")
                        .args(&["show", &oid.to_string()])
                        .status();
                    if let Err(e) = status {
                        // This indicates that tig is not installed, not that the exit
                        // code was non-zero.
                        error!("{}", e);
                        error!("Make sure 'tig' is installed and in $PATH");
                    }
                }
                "" => (), // loop
                x => println!("Didn't understand command: {}", x),
            }
        };
        if let Some(note) = new_note.as_ref() {
            add_note(repo, oid, &note)?;
        }
    }
    Ok(())
}

fn next(repo: &Repository, range: Option<String>) -> anyhow::Result<()> {
    let mut last = None;
    walk_new(&repo, range.as_ref(), |oid| last = Some(oid))?;
    match last {
        Some(oid) => show_commit_with_diffstat(&repo, oid)?,
        None => println!("Everything looks good!"),
    }
    Ok(())
}

fn list(repo: &Repository, range: Option<String>) -> anyhow::Result<()> {
    walk_new(&repo, range.as_ref(), |oid| println!("{}", oid))
}

fn show(repo: &Repository, revspec: &str) -> anyhow::Result<()> {
    let oid = repo.revparse_single(revspec)?.peel_to_commit()?.id();
    let status = lookup(&repo, oid)?;
    println!("{} {} {:?}", revspec, oid, status);
    Ok(())
}

fn add_note(repo: &Repository, oid: Oid, new_note: &str) -> anyhow::Result<()> {
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

/*************************************************************************************/

const NOTES_REF: &str = "refs/notes/orpa";

fn get_note(repo: &Repository, oid: Oid) -> anyhow::Result<Option<String>> {
    match repo.find_note(Some(NOTES_REF), oid) {
        Ok(note) => Ok(note.message().map(|x| x.to_owned())),
        Err(e) if e.code() == ErrorCode::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn lookup(repo: &Repository, oid: Oid) -> anyhow::Result<Status> {
    match get_note(repo, oid)? {
        Some(note) if note == "checkpoint" => Ok(Status::Checkpoint),
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

fn walk_new(
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

fn time_to_chrono(time: Time) -> chrono::NaiveDateTime {
    // FIXME: Include timezone
    NaiveDateTime::from_timestamp(time.seconds(), 0)
}

fn show_commit_oneline(repo: &Repository, oid: Oid) -> anyhow::Result<()> {
    let c = repo.find_commit(oid)?;
    // FIXME: Stats are wrong for merge commits
    let diff = commit_diff(repo, &c)?;
    let stats = diff.stats()?.to_buf(DiffStatsFormat::SHORT, 20)?;
    println!(
        "{} {:<80} {}",
        c.as_object().short_id()?.as_str().unwrap_or("").yellow(),
        c.summary().unwrap_or(""),
        stats.as_str().unwrap_or("").trim().blue(),
    );
    Ok(())
}

/// The diff of a commit against its first parent
fn commit_diff<'a>(repo: &'a Repository, c: &Commit) -> anyhow::Result<Diff<'a>> {
    let base = match c.parent(0) {
        Ok(parent) => parent.tree()?,
        Err(e) if e.code() == ErrorCode::NotFound => empty_tree(repo)?,
        Err(e) => Err(e)?,
    };
    Ok(repo.diff_tree_to_tree(Some(&base), Some(&c.tree()?), None)?)
}

fn empty_tree(repo: &Repository) -> anyhow::Result<Tree> {
    let oid = repo.treebuilder(None)?.write()?;
    Ok(repo.find_tree(oid)?)
}

fn show_commit_with_diffstat(repo: &Repository, oid: Oid) -> anyhow::Result<()> {
    let c = repo.find_commit(oid)?;
    println!("{}{}", "commit ".yellow(), oid.to_string().yellow());
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
enum Status {
    Reviewed,
    Checkpoint,
    Ours,
    Merge,
    New,
}
