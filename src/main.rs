mod mr_db;
mod review_db;

use crate::mr_db::RevInfo;
use crate::review_db::*;
use anyhow::anyhow;
use git2::{Oid, Repository};
use gitlab::{Gitlab, MergeRequest, MergeRequestStateFilter, ProjectId};
use once_cell::sync::Lazy;
use std::io::{stdin, stdout, BufRead, Write};
use std::{fs::File, path::PathBuf, process::Command};
use structopt::StructOpt;
use tracing::*;
use yansi::Paint;

pub static OPTS: Lazy<Opts> = Lazy::new(|| Opts::from_args());

/// A tool for tracking private code review
#[derive(StructOpt, Debug)]
pub struct Opts {
    #[structopt(subcommand)]
    pub cmd: Option<Cmd>,
    #[structopt(long)]
    pub db: Option<std::path::PathBuf>,
    #[structopt(long)]
    pub notes_ref: Option<String>,
}
#[derive(StructOpt, Debug, Clone)]
pub enum Cmd {
    /// Summarize the review status
    Status { range: Option<String> },
    /// Interactively review waiting commits
    #[structopt(alias = "r")]
    Review { range: Option<String> },
    /// Inspect the oldest unreviewed commit
    Next { range: Option<String> },
    /// List all unreviewed commits
    List { range: Option<String> },
    /// Show the status of a commit
    Show {
        /// The commit to show the status of.  It can be a revision such as
        /// "c13f2b6", or a ref such as "origin/master" or "HEAD".
        revspec: String,
    },
    /// Attach a note to a commit
    ///
    /// The provided note will be formatted as a so-called "trailer",
    /// so you probably want to enter a past participle.  Eg. the command
    /// `orpa mark HEAD Tested` will attach the following note to HEAD:
    /// "Tested-by: Joe Smith <joe@smith.net>".  If no note is provided,
    /// the verb "Reviewed" is used.
    Mark {
        /// The commit to attach a note to.  It can be a revision such as
        /// "c13f2b6", or a ref such as "origin/master" or "HEAD".
        revspec: String,
        /// The note to attach.
        note: Option<String>,
    },
    /// Approve a commit and all its ancestors
    Checkpoint {
        /// The commit to mark as a checkpoint.  It can be a revision such as
        /// "c13f2b6", or a ref such as "origin/master" or "HEAD".
        revspec: String,
    },
    /// Speed up future operations
    GC,
    /// Sync MRs from gitlab
    Fetch,
    /// Show a specific merge request
    Mr {
        /// The merge request to show.  Must be an integer.  It can optionally
        /// be prefixed with a '!'.
        id: String,
    },
    /// Show merge requests
    ///
    /// The user's own MRs are hidden by default, as are WIP MRs.
    Mrs {
        /// Include hidden MRs.
        #[structopt(long, short)]
        all: bool,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let repo = Repository::open_from_env()?;
    match OPTS.cmd.clone() {
        None => summary(&repo, None),
        Some(Cmd::Status { range }) => summary(&repo, range),
        Some(Cmd::Review { range }) => review(&repo, range),
        Some(Cmd::Next { range }) => next(&repo, range),
        Some(Cmd::List { range }) => list(&repo, range),
        Some(Cmd::Show { revspec }) => show(&repo, &revspec),
        Some(Cmd::Mark { revspec, note }) => add_note(
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
        Some(Cmd::Fetch) => fetch(&repo),
        Some(Cmd::Mr { id }) => merge_request(&repo, id),
        Some(Cmd::Mrs { all }) => merge_requests(&repo, all),
    }
}

fn summary(repo: &Repository, range: Option<String>) -> anyhow::Result<()> {
    let mut new = vec![];
    walk_new(&repo, range.as_ref(), |oid| new.push(oid))?;
    let n_new = new.len();
    let current = range.as_ref().map_or("Current branch", |x| x.as_str());
    if n_new == 0 {
        println!("{}: no unreviewed commits", current);
    } else {
        println!("{}: The following commits are awaiting review:\n", current);
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
        println!("\nReview them using \"orpa review{}\"", args);
        if n_new > 20 {
            println!("\nHint: That's a lot of unreviewed commits! You can skip old\nones by setting a checkpoint:    orpa checkpoint <oid>");
        }
    }

    if let Ok((mrs, db)) = cached_mrs(repo) {
        let config = repo.config()?;
        let me = config.get_string("gitlab.username")?;

        let mut visible_mrs = vec![];
        for mr in mrs
            .iter()
            .filter(|mr| !(mr.work_in_progress || mr.author.username == me))
        {
            let latest_rev = db.get_revs(mr).last().unwrap()?;
            let range = format!("{}..{}", latest_rev.base, latest_rev.head);
            let mut n_unreviewed = 0;
            review_db::walk_new(&repo, Some(&range), |_| {
                n_unreviewed += 1;
            })?;
            if n_unreviewed > 0 {
                visible_mrs.push((mr, n_unreviewed));
            }
        }

        if visible_mrs.len() > 0 {
            println!("\nMerge requests with unreviewed commits:\n");
        }
        for (mr, n_unreviewed) in visible_mrs.iter().take(10) {
            if mr.assignees.iter().flatten().any(|x| x.username == me) {
                println!(
                    "    {}{:<5} {} ({} unreviewed)",
                    Paint::yellow("!").bold(),
                    Paint::yellow(mr.iid.value()).bold(),
                    Paint::new(&mr.title).bold(),
                    Paint::new(n_unreviewed),
                );
            } else {
                println!(
                    "    {}{:<5} {} ({} unreviewed)",
                    Paint::yellow("!"),
                    Paint::yellow(mr.iid.value()),
                    &mr.title,
                    n_unreviewed,
                );
            }
        }
        if visible_mrs.len() > 10 {
            println!(
                "...and {} more (use \"orpa mrs\" to see them)",
                visible_mrs.len() - 10,
            );
        }
        if visible_mrs.len() > 0 {
            println!("\nUse \"orpa mr <id>\" to see the full MR information");
        }
    }
    Ok(())
}

fn review(repo: &Repository, range: Option<String>) -> anyhow::Result<()> {
    let mut new = vec![];
    walk_new(&repo, range.as_ref(), |oid| new.push(oid))?;
    for oid in new.into_iter().rev() {
        let run_tig = || {
            let status = Command::new("tig")
                .args(&["show", &oid.to_string()])
                .status();
            if let Err(e) = status {
                // This indicates that tig is not installed, not that the exit
                // code was non-zero.
                error!("{}", e);
                error!("Make sure 'tig' is installed and in $PATH");
            }
        };
        run_tig();
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
                "tig" => run_tig(),
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

fn fetch(repo: &Repository) -> anyhow::Result<()> {
    info!("Loading the config");
    let config = repo.config()?;
    let gitlab_host = config.get_string("gitlab.url")?;
    let gitlab_token = config.get_string("gitlab.privateToken")?;
    let project_id = ProjectId::new(config.get_i64("gitlab.projectId")? as u64);

    info!("Opening the database");
    let db_path = db_path(repo);
    let db = mr_db::Db::open(&db_path)?;

    info!("Connecting to gitlab at {}", &gitlab_host);
    let gl = Gitlab::new_insecure(&gitlab_host, &gitlab_token).unwrap();

    println!(
        "Fetching open MRs for project {} from {}...",
        project_id, gitlab_host
    );
    let mrs = gl.merge_requests_with_state(project_id, MergeRequestStateFilter::Opened)?;
    let mr_cache_path = db_path.join("mr_cache");
    serde_json::to_writer(File::create(mr_cache_path)?, &mrs)?;

    info!("Updating the DB with new revisions");
    for mr in &mrs {
        if let Some(info) = db.insert_if_newer(&repo, &gl, project_id, mr)? {
            println!("Updated !{} to v{}", mr.iid.value(), info.rev + 1);
        }
    }

    Ok(())
}

fn db_path(repo: &Repository) -> PathBuf {
    OPTS.db.clone().unwrap_or_else(|| repo.path().join("orpa"))
}

fn cached_mrs(repo: &Repository) -> anyhow::Result<(Vec<MergeRequest>, mr_db::Db)> {
    let db_path = db_path(repo);
    let db = mr_db::Db::open(&db_path)?;
    let mr_cache_path = db_path.join("mr_cache");
    let mrs: Vec<_> = serde_json::from_reader(File::open(mr_cache_path)?)?;
    Ok((mrs, db))
}

fn merge_request(repo: &Repository, target: String) -> anyhow::Result<()> {
    let target: u64 = target.trim_matches(|c: char| !c.is_numeric()).parse()?;
    let config = repo.config()?;
    let me = config.get_string("gitlab.username")?;
    let (mrs, db) = cached_mrs(repo)?;
    if let Some(mr) = mrs.iter().find(|mr| mr.iid.value() == target) {
        print_mr(&me, &mr);
        for x in db.get_revs(mr) {
            print_rev(&repo, x?)?;
        }
        println!();
    }
    Ok(())
}

fn merge_requests(repo: &Repository, include_all: bool) -> anyhow::Result<()> {
    let config = repo.config()?;
    let me = config.get_string("gitlab.username")?;
    let (mrs, db) = cached_mrs(repo)?;
    for mr in mrs
        .iter()
        .filter(|mr| include_all || (!mr.work_in_progress && mr.author.username != me))
    {
        print_mr(&me, &mr);
        for x in db.get_revs(mr) {
            print_rev(&repo, x?)?;
        }
        println!();
    }
    Ok(())
}

fn print_rev(repo: &Repository, rev: RevInfo) -> anyhow::Result<()> {
    let RevInfo { rev, base, head } = rev;
    let range = format!("{}..{}", base, head);
    let mut walk_all = repo.revwalk()?;
    walk_all.push_range(&range)?;
    let n_total = walk_all.count();
    let mut n_unreviewed = 0;
    review_db::walk_new(&repo, Some(&range), |_| {
        n_unreviewed += 1;
    })?;
    let unreviewed_msg = if n_unreviewed == 0 {
        "".into()
    } else {
        format!(
            " ({}/{} reviewed)",
            Paint::new(n_total - n_unreviewed).bold(),
            n_total,
        )
    };
    println!();
    let base = repo.find_commit(base)?;
    let head = repo.find_commit(head)?;
    println!(
        "    v{} {}..{}{}",
        rev + 1,
        Paint::blue(base.as_object().short_id()?.as_str().unwrap_or("")),
        Paint::magenta(head.as_object().short_id()?.as_str().unwrap_or("")),
        unreviewed_msg,
    );
    Ok(())
}

fn print_mr(me: &str, mr: &MergeRequest) {
    println!(
        "{}{}",
        Paint::yellow("merge_request !"),
        Paint::yellow(mr.iid.value())
    );
    println!("Author: {} (@{})", &mr.author.name, &mr.author.username);
    println!("Date:   {}", &mr.updated_at);
    println!();
    println!("    {}", &mr.title);

    if let Some(desc) = mr.description.as_ref() {
        if desc != "" {
            println!();
            for line in desc.lines() {
                println!("    {}", line);
            }
        }
    }

    let mut assignees = mr.assignees.iter().flatten().peekable();
    if assignees.peek().is_some() {
        println!();
        for assignee in assignees {
            let mut s = Paint::new(format!("{} (@{})", assignee.name, assignee.username));
            if assignee.username == me {
                s = s.bold();
            }
            println!("    Assigned-to: {}", s);
        }
    }
}
