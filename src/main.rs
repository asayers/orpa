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
    /// Inspect the oldest unapproved commit
    Next,
    /// List all unapproved commits
    List {
        #[structopt(long)]
        all: bool,
    },
    /// Show the status of a commit
    Show {
        revspec: String,
    },
    /// Approve a commit
    Approve {
        revspec: String,
    },
    /// Approve a commit and all its ancestors
    Checkpoint {
        revspec: String,
    },
    /// Speed up future operations
    GC,
}

fn main() {
    let opts = Opts::from_args();
    tracing_subscriber::fmt::init();
    let res = match opts.cmd {
        None => summary(),
        Some(Cmd::Next) => next(),
        Some(Cmd::List { all }) => list(all),
        Some(Cmd::Show { revspec }) => show(&revspec),
        Some(Cmd::Approve { revspec }) => set_status(&revspec, Status::Approved),
        Some(Cmd::Checkpoint { revspec }) => set_status(&revspec, Status::Checkpoint),
        Some(Cmd::GC) => Err(anyhow!("Auto-checkpointing not implemented yet")),
    };
    match res {
        Ok(()) => (),
        Err(e) => {
            error!("{}", e);
            std::process::exit(1);
        }
    }
}

fn summary() -> anyhow::Result<()> {
    let repo = Repository::open_from_env()?;
    let mut walk = repo.revwalk()?;
    walk.push_head()?;
    let mut unapproved = vec![];
    for oid in walk {
        let oid = oid?;
        let status = lookup(&repo, oid)?;
        match status {
            Status::NotApproved => unapproved.push(oid),
            Status::Checkpoint => break,
            _ => (),
        }
    }
    let n_unapproved = unapproved.len();
    if n_unapproved == 0 {
        println!("Everything looks good!");
    } else {
        println!("The following commits are awaiting approval:\n");
        for oid in unapproved.into_iter().rev().take(10) {
            let c = repo.find_commit(oid)?;
            println!(
                "{} {:<80} {} {}",
                c.as_object().short_id()?.as_str().unwrap_or("").yellow(),
                c.summary().unwrap_or(""),
                time_to_chrono(c.author().when()).to_string().blue(),
                c.author().name().unwrap_or("").green(),
            );
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

fn next() -> anyhow::Result<()> {
    let repo = Repository::open_from_env()?;
    let mut walk = repo.revwalk()?;
    walk.push_head()?;
    let mut last = None;
    for oid in walk {
        let oid = oid?;
        let status = lookup(&repo, oid)?;
        match status {
            Status::NotApproved => last = Some(oid),
            Status::Checkpoint => break,
            _ => (),
        }
    }
    if let Some(oid) = last {
        show_commit_with_diffstat(&repo, oid)?;
        println!();
        let approve = loop {
            print!("Approve? [y/N] [t=>tig] ");
            stdout().flush()?;
            let mut l = String::new();
            stdin().lock().read_line(&mut l)?;
            match l.trim() {
                "y" | "Y" => break true,
                "n" | "N" | "" | "q" => break false,
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
                _ => (),
            }
        };
        if approve {
            let sig = repo.signature()?;
            let status = Status::Approved;
            repo.note(&sig, &sig, Some(NOTES_REF), oid, status.as_str(), false)?;
            println!("Marked {} as {}", oid, status.as_str());
        }
    } else {
        println!("Everything looks good!");
    }
    Ok(())
}

fn time_to_chrono(time: Time) -> chrono::NaiveDateTime {
    // FIXME: Include timezone
    NaiveDateTime::from_timestamp(time.seconds(), 0)
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

fn list(all: bool) -> anyhow::Result<()> {
    let repo = Repository::open_from_env()?;
    let mut walk = repo.revwalk()?;
    walk.push_head()?;
    for oid in walk {
        let oid = oid?;
        let status = lookup(&repo, oid)?;
        if all {
            println!("{}: {}", oid, status.as_str());
        } else {
            match status {
                Status::NotApproved => println!("{}", oid),
                Status::Checkpoint => return Ok(()),
                _ => (),
            }
        }
    }
    Ok(())
}

fn lookup(repo: &Repository, oid: Oid) -> anyhow::Result<Status> {
    match repo.find_note(Some(NOTES_REF), oid) {
        Ok(note) => note.message().unwrap_or("").parse(),
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
#[derive(Copy, Clone, PartialEq, Debug)]
enum Status {
    Approved,
    Checkpoint,
    Ours,
    Merge,
    NotApproved,
}
impl Status {
    fn as_str(&self) -> &'static str {
        match self {
            Status::Approved => "Approved",
            Status::Checkpoint => "Checkpoint",
            Status::Ours => "Ours",
            Status::Merge => "Merge",
            Status::NotApproved => "NotApproved",
        }
    }
}
impl FromStr for Status {
    type Err = anyhow::Error;
    fn from_str(x: &str) -> anyhow::Result<Status> {
        match x {
            "Approved" => Ok(Status::Approved),
            "Checkpoint" => Ok(Status::Checkpoint),
            "Ours" => Ok(Status::Ours),
            "Merge" => Ok(Status::Merge),
            "NotApproved" => Ok(Status::NotApproved),
            _ => bail!("Unknown status: {}", x),
        }
    }
}

const NOTES_REF: &str = "refs/notes/approvals";

fn show(revspec: &str) -> anyhow::Result<()> {
    let repo = Repository::open_from_env()?;
    let oid = repo.revparse_single(revspec)?.peel_to_commit()?.id();
    let status = lookup(&repo, oid)?;
    println!("{} {} {:?}", revspec, oid, status);
    Ok(())
}

fn set_status(revspec: &str, status: Status) -> anyhow::Result<()> {
    let repo = Repository::open_from_env()?;
    let oid = repo.revparse_single(revspec)?.peel_to_commit()?.id();
    let sig = repo.signature()?;
    repo.note(&sig, &sig, Some(NOTES_REF), oid, status.as_str(), false)?;
    println!("Marked {} as {}", oid, status.as_str());
    Ok(())
}
