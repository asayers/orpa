mod fetch;
mod mr_db;
mod review_db;

use crate::fetch::fetch;
use crate::mr_db::{Version, VersionInfo};
use crate::review_db::*;
use anyhow::anyhow;
use clap::Parser;
use git2::{Oid, Repository};
use gitlab::{MergeRequest, MergeRequestState, ProjectId};
use globset::GlobSet;
use once_cell::sync::{Lazy, OnceCell};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::{fs::File, path::PathBuf};
use tabwriter::TabWriter;
use tracing::*;
use yansi::Paint;

pub static OPTS: Lazy<Opts> = Lazy::new(Opts::from_args);

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
    /// Summarize the review status of a branch
    Branch {
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
        let idx = LineIdx::open(&db_path(repo))?;
        idx.refresh(repo)?;
        Ok(idx)
    })
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing_subscriber::filter::LevelFilter::WARN.into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let repo = Repository::open_from_env()?;
    match OPTS.cmd.clone() {
        None => summary(&repo),
        Some(Cmd::Branch { range }) => branch(&repo, range),
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

fn load_watchlist(repo: &Repository) -> anyhow::Result<GlobSet> {
    use globset::*;
    let mut watchlist = GlobSetBuilder::new();
    if let Ok(file) = File::open(db_path(repo).join("watchlist")) {
        for line in BufReader::new(file).lines() {
            watchlist.add(Glob::new(&line?)?);
        }
    }
    Ok(watchlist.build()?)
}

fn summary(repo: &Repository) -> anyhow::Result<()> {
    let db = mr_db::Db::open(&db_path(repo))?;
    if let Ok(mrs) = cached_mrs(repo) {
        let config = repo.config()?;
        let me = config.get_string("gitlab.username")?;

        let watchlist = load_watchlist(repo)?;

        let mut visible_mrs = vec![];
        let mut n_hidden = 0;
        for mr in mrs.iter().filter(|mr| mr.author.username != me) {
            let mut f = || {
                let latest_rev = db
                    .latest_version(mr)?
                    .ok_or_else(|| anyhow!("Can't find any versions"))?;
                let n_unreviewed = version_stats(repo, latest_rev)?[Status::New];
                if n_unreviewed == 0 {
                    return Ok(());
                }

                let assigned = mr
                    .assignee
                    .iter()
                    .chain(mr.assignees.iter().flatten())
                    .chain(mr.reviewers.iter().flatten())
                    .any(|x| x.username == me);
                let watchlist_hit = mr_paths(repo, latest_rev)?
                    .iter()
                    .any(|path| watchlist.is_match(path));
                let partially_reviewed = db
                    .get_versions(mr)
                    .flat_map(|ver| version_stats(repo, ver?))
                    .any(|stats| stats[Status::Reviewed] > 0);
                let is_interesting = assigned || watchlist_hit || partially_reviewed;

                if is_interesting {
                    visible_mrs.push((mr, n_unreviewed, is_interesting));
                } else {
                    let too_old = chrono::Utc::now() - mr.updated_at > chrono::Duration::weeks(13);
                    let too_many = visible_mrs.len() >= 20;
                    if too_old || too_many {
                        n_hidden += 1;
                    } else {
                        visible_mrs.push((mr, n_unreviewed, is_interesting));
                    }
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
        let all_clear = visible_mrs.is_empty();

        if !all_clear {
            println!("Merge requests with unreviewed commits:\n");
        }
        let mut tw = TabWriter::new(std::io::stdout()).ansi(true);
        for (mr, n_unreviewed, is_interesting) in visible_mrs {
            let when = timeago::Formatter::new().convert_chrono(mr.updated_at, chrono::Utc::now());
            if is_interesting {
                writeln!(
                    tw,
                    "  {}{}\t{}\t{}\t{}\t({} unreviewed)",
                    Paint::yellow("!").bold(),
                    Paint::yellow(mr.iid.value()).bold(),
                    Paint::blue(&when).bold(),
                    Paint::green(&mr.author.username).bold(),
                    Paint::new(&mr.title).bold(),
                    Paint::new(n_unreviewed),
                )?;
            } else {
                writeln!(
                    tw,
                    "  {}{}\t{}\t{}\t{}",
                    Paint::yellow("!"),
                    Paint::yellow(mr.iid.value()),
                    Paint::blue(&when),
                    Paint::green(&mr.author.username).italic(),
                    &mr.title,
                )?;
            }
        }
        tw.flush()?;
        if n_hidden > 0 {
            println!("...and {n_hidden} more (use \"orpa mrs\" to see them)");
        }
        if !all_clear {
            println!("\nUse \"orpa mr <id>\" to see the full MR information");
        }
    }
    Ok(())
}

fn branch(repo: &Repository, range: Option<String>) -> anyhow::Result<()> {
    let mut new = vec![];
    walk_new(repo, range.as_ref(), |oid| new.push(oid))?;
    let n_new = new.len();
    let current = range.as_ref().map_or("Current branch", |x| x.as_str());
    if n_new == 0 {
        println!("{}: no unreviewed commits", current);
    } else {
        println!("{}: The following commits are awaiting review:\n", current);
        for oid in new.into_iter().rev().take(10) {
            show_commit_oneline(repo, oid)?;
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
    Ok(())
}

fn next(repo: &Repository, range: Option<String>) -> anyhow::Result<()> {
    let mut last = None;
    walk_new(repo, range.as_ref(), |oid| last = Some(oid))?;
    match last {
        Some(oid) => show_commit_with_diffstat(repo, oid)?,
        None => println!("Everything looks good!"),
    }
    Ok(())
}

fn list(repo: &Repository, range: Option<String>) -> anyhow::Result<()> {
    walk_new(repo, range.as_ref(), |oid| println!("{}", oid))
}

fn show(repo: &Repository, revspec: &str) -> anyhow::Result<()> {
    let oid = repo.revparse_single(revspec)?.peel_to_commit()?.id();
    let status = lookup(repo, oid)?;
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
            host: config
                .get_string("gitlab.url")
                .unwrap_or_else(|_| "gitlab.com".into()),
            project_id: ProjectId::new(config.get_i64("gitlab.projectId")? as u64),
            token: config.get_string("gitlab.privateToken")?,
        })
    }
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
    mrs.sort_by_key(|mr| std::cmp::Reverse(mr.updated_at));
    Ok(mrs)
}

fn merge_request(repo: &Repository, target: String) -> anyhow::Result<()> {
    pager::Pager::with_pager("less -FRSX").setup();
    let target = target.trim_matches(|c: char| !c.is_numeric());
    let path = db_path(repo).join("merge_requests").join(target);
    let mr: MergeRequest = serde_json::from_reader(File::open(path)?)?;

    let db = mr_db::Db::open(&db_path(repo))?;
    let config = repo.config()?;
    let me = config.get_string("gitlab.username")?;
    print_mr(&me, &mr);
    println!();
    let vers = db.get_versions(&mr).collect::<anyhow::Result<Vec<_>>>()?;
    let n_vers = vers.len();
    for (i, version) in vers.into_iter().enumerate() {
        print_version(repo, version, i + 1 == n_vers)?;
    }
    Ok(())
}

fn merge_requests(repo: &Repository, include_all: bool) -> anyhow::Result<()> {
    pager::Pager::with_pager("less -FRSX").setup();
    let config = repo.config()?;
    let me = config.get_string("gitlab.username")?;
    let db = mr_db::Db::open(&db_path(repo))?;
    let mut mrs = cached_mrs(repo)?;
    mrs.retain(|mr| include_all || (!mr.draft && mr.author.username != me));
    for mr in mrs {
        print_mr(&me, &mr);
        println!();
        let vers = db.get_versions(&mr).collect::<anyhow::Result<Vec<_>>>()?;
        let n_vers = vers.len();
        for (i, version) in vers.into_iter().enumerate() {
            print_version(repo, version, i + 1 == n_vers)?;
        }
        println!();
    }
    Ok(())
}

fn similar(repo: &Repository, revspec: &str) -> anyhow::Result<()> {
    let commit = repo.revparse_single(revspec)?.peel_to_commit()?;
    for (oid, x) in similiar_commits(repo, &commit)?.into_iter().take(10) {
        println!("{} (similarity: {:.02}%)", oid, x.score() * 100.);
    }
    Ok(())
}

fn print_version(repo: &Repository, version: VersionInfo, is_last: bool) -> anyhow::Result<()> {
    let (base, head) = match repo
        .find_commit(version.base)
        .and_then(|x| repo.find_commit(version.head).map(|y| (x, y)))
    {
        Ok(x) => x,
        Err(_) => {
            let base = &version.base.to_string()[..7];
            let head = &version.head.to_string()[..7];
            println!(
                "    {} {}..{} (commits missing)",
                version.version,
                Paint::blue(base),
                Paint::magenta(head),
            );
            return Ok(());
        }
    };

    {
        let base = base.as_object().short_id()?;
        let head = head.as_object().short_id()?;
        print!(
            "    {} {}..{}",
            version.version,
            Paint::blue(base.as_str().unwrap_or("")),
            Paint::magenta(head.as_str().unwrap_or("")),
        );
    }

    let (n_unreviewed, n_total) = count_reviewed(repo, version)?;
    if n_unreviewed != 0 {
        print!(
            " ({}/{} reviewed)",
            Paint::new(n_total - n_unreviewed).bold(),
            n_total,
        );
    }
    println!();

    if is_last {
        let diff = repo.diff_tree_to_tree(Some(&base.tree()?), Some(&head.tree()?), None)?;
        println!();
        print_diff_stat(diff)?;
    }

    Ok(())
}

fn print_diff_stat(diff: git2::Diff) -> anyhow::Result<()> {
    let stats = diff.stats()?.to_buf(git2::DiffStatsFormat::FULL, 100)?;
    for l in stats.as_str().unwrap().lines() {
        match l.split_once('|') {
            None => println!("{}", l),
            Some((path, change)) => {
                let change = change
                    .replace('+', &Paint::green("+").to_string())
                    .replace('-', &Paint::red("-").to_string());
                println!("{}|{}", path, change);
            }
        }
    }
    Ok(())
}

fn count_reviewed(repo: &Repository, version: VersionInfo) -> anyhow::Result<(usize, usize)> {
    let range = format!("{}..{}", version.base, version.head);
    let mut walk_all = repo.revwalk()?;
    walk_all.push_range(&range)?;
    let n_total = walk_all.count();
    let mut n_unreviewed = 0;
    walk_new(repo, Some(&range), |_| {
        n_unreviewed += 1;
    })?;
    Ok((n_unreviewed, n_total))
}

pub fn fmt_state(x: MergeRequestState) -> &'static str {
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
        if !desc.is_empty() {
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

/// Paths changed by an MR
fn mr_paths(repo: &Repository, mr: VersionInfo) -> anyhow::Result<Vec<PathBuf>> {
    let base = repo.find_commit(mr.base)?.tree()?;
    let head = repo.find_commit(mr.head)?.tree()?;
    let diff = repo.diff_tree_to_tree(Some(&base), Some(&head), None)?;
    let mut paths = HashSet::<&Path>::default();
    for delta in diff.deltas() {
        if let Some(path) = delta.new_file().path() {
            paths.insert(path);
        }
    }
    Ok(paths.into_iter().map(|x| x.to_path_buf()).collect())
}
