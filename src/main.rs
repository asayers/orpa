use anyhow::{anyhow, bail};
use chrono::NaiveDateTime;
use colored::Colorize;
use git2::{DiffStatsFormat, ErrorCode, Oid, Repository, Time};
use std::io::{stdin, stdout, BufRead, Write};
use std::process::Command;
use std::str::FromStr;
use structopt::StructOpt;
use tracing::*;

#[derive(StructOpt)]
struct Opts {
    #[structopt(subcommand)]
    cmd: Option<Cmd>,
}
#[derive(StructOpt)]
enum Cmd {
    /// Interactively approve waiting commits
    Triage,
    /// Inspect the oldest unapproved commit
    Next,
    /// List all unapproved commits
    List,
    /// Show the status of a commit
    Show { revspec: String },
    /// Approve a commit
    Approve { revspec: String },
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
        None => summary(&repo),
        Some(Cmd::Triage) => triage(&repo),
        Some(Cmd::Next) => next(&repo),
        Some(Cmd::List) => list(&repo),
        Some(Cmd::Show { revspec }) => show(&repo, &revspec),
        Some(Cmd::Approve { revspec }) => set_note(&repo, &revspec, OrpaNote::Approved),
        Some(Cmd::Checkpoint { revspec }) => set_note(&repo, &revspec, OrpaNote::Checkpoint),
        Some(Cmd::GC) => Err(anyhow!("Auto-checkpointing not implemented yet")),
    }
}

fn summary(repo: &Repository) -> anyhow::Result<()> {
    let mut unapproved = vec![];
    walk_unapproved(&repo, |oid| unapproved.push(oid))?;
    let n_unapproved = unapproved.len();
    if n_unapproved == 0 {
        println!("Everything looks good!");
    } else {
        println!("The following commits are awaiting approval:\n");
        for oid in unapproved.into_iter().rev().take(10) {
            show_commit_oneline(&repo, oid)?;
        }
        if n_unapproved > 10 {
            println!(
                "...and {} more (use \"orpa list\" to see them)",
                n_unapproved - 10
            );
            println!("\nHint: You have a lot of unapproved commits. You can skip old\nones by setting a checkpoint:    orpa checkpoint <oid>");
        }
    }
    Ok(())
}

fn triage(repo: &Repository) -> anyhow::Result<()> {
    let mut unapproved = vec![];
    walk_unapproved(&repo, |oid| unapproved.push(oid))?;
    for oid in unapproved.into_iter().rev() {
        show_commit_with_diffstat(&repo, oid)?;
        println!();
        let approve = loop {
            print!("Approve? [y/N] [t=>tig] [q=>quit] ");
            stdout().flush()?;
            let mut l = String::new();
            stdin().lock().read_line(&mut l)?;
            match l.trim() {
                "y" | "Y" => break true,
                "n" | "N" | "" => break false,
                "t" | "T" => {
                    let status = Command::new("tig")
                        .args(&["show", &oid.to_string()])
                        .status();
                    if status.is_err() {
                        // This indicates that tig is not installed, not that the exit
                        // code was non-zero.
                        error!("Command not found.  Try installing the 'tig' package");
                    }
                }
                "q" | "Q" => return Ok(()),
                _ => (),
            }
        };
        if approve {
            let sig = repo.signature()?;
            let note = OrpaNote::Approved;
            repo.note(&sig, &sig, Some(NOTES_REF), oid, note.as_str(), false)?;
            println!("Marked {} as {}", oid, note.as_str());
        }
    }
    Ok(())
}

fn next(repo: &Repository) -> anyhow::Result<()> {
    let mut last = None;
    walk_unapproved(&repo, |oid| last = Some(oid))?;
    match last {
        Some(oid) => show_commit_with_diffstat(&repo, oid)?,
        None => println!("Everything looks good!"),
    }
    Ok(())
}

fn list(repo: &Repository) -> anyhow::Result<()> {
    walk_unapproved(&repo, |oid| println!("{}", oid))
}

fn show(repo: &Repository, revspec: &str) -> anyhow::Result<()> {
    let oid = repo.revparse_single(revspec)?.peel_to_commit()?.id();
    let status = lookup(&repo, oid)?;
    println!("{} {} {:?}", revspec, oid, status);
    Ok(())
}

fn set_note(repo: &Repository, revspec: &str, note: OrpaNote) -> anyhow::Result<()> {
    let oid = repo.revparse_single(revspec)?.peel_to_commit()?.id();
    let sig = repo.signature()?;
    repo.note(&sig, &sig, Some(NOTES_REF), oid, note.as_str(), false)?;
    println!("Marked {} as {}", oid, note.as_str());
    Ok(())
}

/*************************************************************************************/

const NOTES_REF: &str = "refs/notes/approvals";

fn lookup(repo: &Repository, oid: Oid) -> anyhow::Result<Status> {
    match repo.find_note(Some(NOTES_REF), oid) {
        Ok(note) => match note.message().unwrap_or("").parse()? {
            OrpaNote::Approved => Ok(Status::Approved),
            OrpaNote::Checkpoint => Ok(Status::Checkpoint),
        },
        Err(e) if e.code() == ErrorCode::NotFound => {
            let commit = repo.find_commit(oid)?;
            let sig = repo.signature()?;
            if commit.author().name_bytes() == sig.name_bytes() {
                Ok(Status::Ours)
            } else if commit.parent_count() > 1 {
                Ok(Status::Merge)
            } else {
                Ok(Status::NotApproved)
            }
        }
        Err(e) => Err(e.into()),
    }
}

fn walk_unapproved(repo: &Repository, mut f: impl FnMut(Oid)) -> anyhow::Result<()> {
    let mut walk = repo.revwalk()?;
    walk.push_head()?;
    for oid in walk {
        let oid = oid?;
        let status = lookup(&repo, oid)?;
        match status {
            Status::NotApproved => f(oid),
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
    let diff = repo.diff_tree_to_tree(Some(&c.parent(0)?.tree()?), Some(&c.tree()?), None)?;
    let stats = diff.stats()?.to_buf(DiffStatsFormat::SHORT, 20)?;
    println!(
        "{} {:<80} {}",
        c.as_object().short_id()?.as_str().unwrap_or("").yellow(),
        c.summary().unwrap_or(""),
        stats.as_str().unwrap_or("").trim().blue(),
    );
    Ok(())
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
    let diff = repo.diff_tree_to_tree(Some(&c.parent(0)?.tree()?), Some(&c.tree()?), None)?;
    let stats = diff.stats()?.to_buf(DiffStatsFormat::FULL, 80)?;
    print!("{}", stats.as_str().unwrap_or(""));
    Ok(())
}

#[derive(Copy, Clone, PartialEq, Debug)]
enum Status {
    Approved,
    Checkpoint,
    Ours,
    Merge,
    NotApproved,
}

#[derive(Copy, Clone, PartialEq, Debug)]
enum OrpaNote {
    Approved,
    Checkpoint,
}

impl OrpaNote {
    fn as_str(&self) -> &'static str {
        match self {
            OrpaNote::Approved => "Approved",
            OrpaNote::Checkpoint => "Checkpoint",
        }
    }
}

impl FromStr for OrpaNote {
    type Err = anyhow::Error;
    fn from_str(x: &str) -> anyhow::Result<OrpaNote> {
        match x {
            "Approved" => Ok(OrpaNote::Approved),
            "Checkpoint" => Ok(OrpaNote::Checkpoint),
            _ => bail!("Unknown status: {}", x),
        }
    }
}
