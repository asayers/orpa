mod mr_db;
mod review_db;

use crate::mr_db::VersionInfo;
use crate::review_db::*;
use anyhow::anyhow;
use clap::Parser;
use git2::{Oid, Repository};
use gitlab::{Gitlab, MergeRequest, MergeRequestInternalId, MergeRequestState, ProjectId};
use once_cell::sync::{Lazy, OnceCell};
use std::collections::HashSet;
use std::{fs::File, path::PathBuf};
use tracing::*;
use yansi::Paint;

pub static OPTS: Lazy<Opts> = Lazy::new(|| Opts::from_args());

/// A tool for tracking private code review
#[derive(Parser, Debug)]
pub struct Opts {
    #[clap(subcommand)]
    pub cmd: Option<Cmd>,
    #[clap(long)]
    pub db: Option<std::path::PathBuf>,
    #[clap(long)]
    pub dedup: bool,
    #[clap(long)]
    pub notes_ref: Option<String>,
}
#[derive(Parser, Debug, Clone)]
pub enum Cmd {
    /// Summarize the review status
    Status {
        range: Option<String>,
    },
    /// Inspect the oldest unreviewed commit
    Next {
        range: Option<String>,
    },
    /// List all unreviewed commits
    List {
        range: Option<String>,
    },
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
        #[clap(long, short)]
        all: bool,
    },
    /// Show recent reviews
    Recent,
    Similar {
        revspec: String,
    },
}

pub fn get_idx(repo: &Repository) -> anyhow::Result<&LineIdx> {
    static LINE_IDX: OnceCell<LineIdx> = OnceCell::new();
    LINE_IDX.get_or_try_init(|| {
        let idx = LineIdx::open(&db_path(&repo))?;
        idx.refresh(&repo)?;
        Ok(idx)
    })
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let repo = Repository::open_from_env()?;
    match OPTS.cmd.clone() {
        None => summary(&repo, None),
        Some(Cmd::Status { range }) => summary(&repo, range),
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
        Some(Cmd::Recent) => {
            for x in review_db::recent_notes(&repo)? {
                println!("{}", x);
            }
            Ok(())
        }
        Some(Cmd::Similar { revspec }) => similar(&repo, &revspec),
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
                "  ...and {} more (use \"orpa list{}\" to see them)",
                n_new - 10,
                args,
            );
        }
        if n_new > 20 {
            println!("\nHint: That's a lot of unreviewed commits! You can skip old\nones by setting a checkpoint:    orpa checkpoint <oid>");
        }
    }

    let db = mr_db::Db::open(&db_path(repo))?;
    if let Ok(mrs) = cached_mrs(repo) {
        let config = repo.config()?;
        let me = config.get_string("gitlab.username")?;

        let mut visible_mrs = vec![];
        for mr in mrs
            .iter()
            .filter(|mr| !(mr.work_in_progress || mr.author.username == me))
        {
            let mut f = || {
                let latest_rev = db
                    .get_versions(mr)
                    .last()
                    .ok_or(anyhow!("Can't find any versions"))??;
                let range = format!("{}..{}", latest_rev.base, latest_rev.head);
                let mut n_unreviewed = 0;
                walk_new(&repo, Some(&range), |_| {
                    n_unreviewed += 1;
                })?;
                if n_unreviewed > 0 {
                    visible_mrs.push((mr, n_unreviewed));
                }
                anyhow::Ok(())
            };
            match f() {
                Ok(()) => (),
                Err(e) => {
                    error!("{}: {}", mr.iid.value(), e);
                    continue;
                }
            }
        }

        if visible_mrs.len() > 0 {
            println!("\nMerge requests with unreviewed commits:\n");
        }
        for (mr, n_unreviewed) in visible_mrs.iter().take(10) {
            if mr.assignees.iter().flatten().any(|x| x.username == me) {
                println!(
                    "  {}{:<6} {} ({} unreviewed)",
                    Paint::yellow("!").bold(),
                    Paint::yellow(mr.iid.value()).bold(),
                    Paint::new(&mr.title).bold(),
                    Paint::new(n_unreviewed),
                );
            } else {
                println!(
                    "  {}{:<6} {}",
                    Paint::yellow("!"),
                    Paint::yellow(mr.iid.value()),
                    &mr.title,
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

pub struct GitlabConfig {
    pub host: String,
    pub project_id: ProjectId,
    pub token: String,
}

impl GitlabConfig {
    fn load(repo: &Repository) -> anyhow::Result<GitlabConfig> {
        info!("Loading the config");
        let config = repo.config()?;
        Ok(GitlabConfig {
            host: config.get_string("gitlab.url")?,
            project_id: ProjectId::new(config.get_i64("gitlab.projectId")? as u64),
            token: config.get_string("gitlab.privateToken")?,
        })
    }
}

fn fetch(repo: &Repository) -> anyhow::Result<()> {
    let config = GitlabConfig::load(repo)?;

    info!("Opening the database");
    let db_path = db_path(repo);
    let db = mr_db::Db::open(&db_path)?;

    info!("Connecting to gitlab at {}", config.host);
    let gl = Gitlab::new(&config.host, &config.token)?;

    println!("Fetching open MRs for project {}...", config.project_id);
    let mrs: Vec<MergeRequest> = {
        use gitlab::api::{projects::merge_requests::*, *};
        let query = MergeRequestsBuilder::default()
            .project(config.project_id.value())
            .state(MergeRequestState::Opened)
            .build()
            .map_err(|e| anyhow!(e))?;
        paged(query, Pagination::All).query(&gl)?
    };

    info!("Caching the MR info");
    let mr_dir = db_path.join("merge_requests");
    std::fs::create_dir_all(&mr_dir)?;
    for mr in &mrs {
        let path = mr_dir.join(mr.iid.to_string());
        serde_json::to_writer(File::create(path)?, &mr)?;
    }

    info!("Updating the DB with new versions");
    for mr in &mrs {
        match db.insert_if_newer(&repo, &gl, config.project_id, mr) {
            Ok(Some(info)) => println!("Updated !{} to {}", mr.iid.value(), info.version),
            Ok(None) => (),
            Err(e) => error!("{}", e),
        }
    }

    info!("Checking in on open MRs we didn't get an update for");
    let mrs: HashSet<MergeRequestInternalId> = mrs.into_iter().map(|mr| mr.iid).collect();
    for entry in std::fs::read_dir(mr_dir)? {
        let entry = entry?;
        let id = MergeRequestInternalId::new(entry.file_name().into_string().unwrap().parse()?);
        if mrs.contains(&id) {
            // We already saw this one, it's still open
            continue;
        }
        let mr: MergeRequest = serde_json::from_reader(File::open(entry.path())?)?;
        if mr.state != MergeRequestState::Opened {
            // This MR is closed, that's why we didn't see it in the results
            continue;
        }
        info!("What has happened to !{}..?", mr.iid.value());
        let q = {
            use gitlab::api::projects::merge_requests::*;
            MergeRequestBuilder::default()
                .project(config.project_id.value())
                .merge_request(mr.id.value())
                .build()?
        };
        use gitlab::api::Query;
        let new_info: MergeRequest = match q.query(&gl) {
            Ok(x) => x,
            Err(gitlab::api::ApiError::Gitlab { msg }) if msg == "404 Not found" => {
                let path = entry.path();
                warn!("MR is gone! Deleting {}...", path.display());
                std::fs::remove_file(path)?;
                continue;
            }
            Err(e) => {
                error!("{}: {}", mr.iid.value(), e);
                continue;
            }
        };
        serde_json::to_writer(File::create(entry.path())?, &new_info)?;
        if let Some(info) = db.insert_if_newer(&repo, &gl, config.project_id, &new_info)? {
            println!("Updated !{} to {}", mr.iid.value(), info.version);
        }
        println!(
            "Status of !{} changed to {}",
            mr.iid,
            fmt_state(new_info.state)
        );
    }

    Ok(())
}

fn db_path(repo: &Repository) -> PathBuf {
    OPTS.db.clone().unwrap_or_else(|| repo.path().join("orpa"))
}

fn cached_mrs(repo: &Repository) -> anyhow::Result<Vec<MergeRequest>> {
    let mr_dir = db_path(repo).join("merge_requests");
    let mut mrs = vec![];
    for entry in std::fs::read_dir(mr_dir)? {
        let mr: MergeRequest = serde_json::from_reader(File::open(entry?.path())?)?;
        mrs.push(mr);
    }
    Ok(mrs)
}

fn merge_request(repo: &Repository, target: String) -> anyhow::Result<()> {
    let target = target.trim_matches(|c: char| !c.is_numeric());
    let path = db_path(repo).join("merge_requests").join(target);
    let mr: MergeRequest = serde_json::from_reader(File::open(path)?)?;

    let db = mr_db::Db::open(&db_path(repo))?;
    let config = repo.config()?;
    let me = config.get_string("gitlab.username")?;
    print_mr(&me, &mr);
    println!();
    for x in db.get_versions(&mr) {
        print_version(&repo, x?)?;
    }
    Ok(())
}

fn merge_requests(repo: &Repository, include_all: bool) -> anyhow::Result<()> {
    let config = repo.config()?;
    let me = config.get_string("gitlab.username")?;
    let db = mr_db::Db::open(&db_path(repo))?;
    let mrs = cached_mrs(repo)?;
    for mr in mrs
        .iter()
        .filter(|mr| include_all || (!mr.work_in_progress && mr.author.username != me))
    {
        print_mr(&me, &mr);
        println!();
        for x in db.get_versions(mr) {
            print_version(&repo, x?)?;
        }
        println!();
    }
    Ok(())
}

fn similar(repo: &Repository, revspec: &str) -> anyhow::Result<()> {
    let commit = repo.revparse_single(&revspec)?.peel_to_commit()?;
    for (oid, x) in similiar_commits(&repo, &commit)?.into_iter().take(10) {
        println!("{} (similarity: {:.02}%)", oid, x.score() * 100.);
    }
    Ok(())
}

fn print_version(repo: &Repository, version: VersionInfo) -> anyhow::Result<()> {
    let VersionInfo {
        version,
        base,
        head,
    } = version;
    let range = format!("{}..{}", base, head);
    let mut walk_all = repo.revwalk()?;
    walk_all.push_range(&range)?;
    let n_total = walk_all.count();
    let mut n_unreviewed = 0;
    walk_new(&repo, Some(&range), |_| {
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
    let base = repo.find_commit(base)?.as_object().short_id()?;
    let base = Paint::blue(base.as_str().unwrap_or(""));
    let head = repo.find_commit(head)?.as_object().short_id()?;
    let head = Paint::magenta(head.as_str().unwrap_or(""));
    println!("    {version} {base}..{head}{unreviewed_msg}");
    Ok(())
}

fn fmt_state(x: MergeRequestState) -> &'static str {
    match x {
        MergeRequestState::Opened => "open",
        MergeRequestState::Closed => "closed",
        MergeRequestState::Reopened => "open",
        MergeRequestState::Merged => "merged",
        MergeRequestState::Locked => "locked",
    }
}

fn print_mr(me: &str, mr: &MergeRequest) {
    println!(
        "{}{} ({} -> {})",
        Paint::yellow("merge_request !"),
        Paint::yellow(mr.iid.value()),
        mr.source_branch,
        mr.target_branch,
    );
    println!("Status: {}", fmt_state(mr.state));
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
