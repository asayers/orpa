use anyhow::anyhow;
use git2::{Oid, Repository};
use review_db::*;
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
    Status { range: Option<String> },
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
        Some(Cmd::Status { range }) => summary(&repo, range),
        Some(Cmd::Triage { range }) => triage(&repo, range),
        Some(Cmd::Next { range }) => next(&repo, range),
        Some(Cmd::List { range }) => list(&repo, range),
        Some(Cmd::Show { revspec }) => show(&repo, &revspec),
        Some(Cmd::Review { revspec, note }) => add_note(
            &repo,
            repo.revparse_single(&revspec)?.peel_to_commit()?.id(),
            note.as_ref().map_or("Reviewed", |x| x.as_str()),
        ),
        Some(Cmd::Checkpoint { revspec }) => append_note(
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
        let args = match range.as_ref() {
            Some(r) => format!(" {}", r),
            None => "".into(),
        };
        if n_new > 10 {
            println!(
                "...and {} more (use \"orpa list{}\" to see them)",
                n_new - 10,
                args,
            );
        }
        println!("\nReview them using \"orpa triage{}\"", args);
        if n_new > 20 {
            println!("\nHint: That's a lot of unreviewed commits! You can skip old\nones by setting a checkpoint:    orpa checkpoint <oid>");
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
                "h" | "help" | "?" => println!("mark      leave a note\nskip      review again next time\ntig       open in tig\nquit      end review session"),
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
    println!("Everything looks good!");
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

fn add_note(repo: &Repository, oid: Oid, verb: &str) -> anyhow::Result<()> {
    let sig = repo.signature()?;
    let new_note = format!(
        "{}-by: {} <{}>",
        verb,
        sig.name().unwrap_or(""),
        sig.email().unwrap_or(""),
    );
    append_note(repo, oid, &new_note)
}
