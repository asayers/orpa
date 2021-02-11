use anyhow::anyhow;
use git2::{Oid, Repository};
use gitlab::{Gitlab, MergeRequest, MergeRequestStateFilter, ProjectId};
use mr_db::RevInfo;
use review_db::*;
use std::io::{stdin, stdout, BufRead, Write};
use std::{fs::File, path::PathBuf, process::Command};
use structopt::StructOpt;
use tracing::*;
use yansi::Paint;

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
    /// Sync MRs from gitlab
    Fetch {
        #[structopt(long)]
        db: Option<std::path::PathBuf>,
    },
    /// Show merge requests
    ///
    /// The user's own MRs are hidden by default, as are WIP MRs.
    Mrs {
        #[structopt(long)]
        db: Option<std::path::PathBuf>,
        /// Include hidden MRs.
        #[structopt(long, short)]
        hidden: bool,
    },
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
        Some(Cmd::Fetch { db }) => fetch(&repo, db),
        Some(Cmd::Mrs { db, hidden }) => merge_requests(&repo, db, hidden),
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

fn fetch(repo: &Repository, db_path: Option<PathBuf>) -> anyhow::Result<()> {
    info!("Loading the config");
    let config = repo.config()?;
    let gitlab_host = config.get_string("gitlab.url")?;
    let gitlab_token = config.get_string("gitlab.privateToken")?;
    let project_id = ProjectId::new(config.get_i64("gitlab.projectId")? as u64);

    info!("Opening the database");
    let db_path = db_path.unwrap_or_else(|| repo.path().join("merge_requests"));
    let db = mr_db::Db::open(&db_path)?;

    info!("Connecting to gitlab at {}", &gitlab_host);
    let gl = Gitlab::new_insecure(&gitlab_host, &gitlab_token).unwrap();

    info!("Fetching all open MRs for project {}", project_id);
    let mrs = gl.merge_requests_with_state(project_id, MergeRequestStateFilter::Opened)?;
    let mr_cache_path = db_path.join("mr_cache");
    serde_json::to_writer(File::create(mr_cache_path)?, &mrs)?;

    info!("Updating the DB with new revisions");
    for mr in &mrs {
        db.insert_if_newer(&repo, &gl, project_id, mr)?;
    }

    Ok(())
}

fn merge_requests(repo: &Repository, db_path: Option<PathBuf>, hidden: bool) -> anyhow::Result<()> {
    info!("Loading the config");
    let config = repo.config()?;
    let me = config.get_string("gitlab.username")?;

    info!("Opening the database");
    let db_path = db_path.unwrap_or_else(|| repo.path().join("merge_requests"));
    let db = mr_db::Db::open(&db_path)?;

    let mr_cache_path = db_path.join("mr_cache");
    info!("Reading cached MRs from {}", mr_cache_path.display());
    let mrs: Vec<MergeRequest> = serde_json::from_reader(File::open(mr_cache_path)?)?;

    info!("Printing MR info");
    for mr in mrs
        .iter()
        .filter(|mr| hidden || (!mr.work_in_progress && mr.author.username != me))
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
        "    rev #{}: {}..{}{}",
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
    println!("Title:  {}", &mr.title);

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
