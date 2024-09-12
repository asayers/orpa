mod fetch;
mod mr_db;
mod review_db;

use crate::fetch::{fetch, MergeRequest, MergeRequestState, ProjectId};
use crate::mr_db::{Version, VersionInfo};
use crate::review_db::*;
use anyhow::anyhow;
use clap::Parser;
use git2::{Commit, Oid, Repository};
use globset::GlobSet;
use mr_db::MRWithVersions;
use once_cell::sync::{Lazy, OnceCell};
use std::collections::HashSet;
use std::io::Write;
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
    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        Paint::disable();
    }
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
    let config = repo.config()?;
    let globs = config.get_string("orpa.watchlist")?;
    let mut watchlist = GlobSetBuilder::new();
    for glob in globs.split(':') {
        watchlist.add(Glob::new(glob)?);
    }
    Ok(watchlist.build()?)
}

fn summary(repo: &Repository) -> anyhow::Result<()> {
    if let Ok(mrs) = cached_mrs(repo) {
        let config = repo.config()?;
        let me = config.get_string("gitlab.username")?;

        let watchlist = load_watchlist(repo)?;

        let mut interesting = vec![];
        let mut recent = vec![];
        let mut drafts = vec![];
        let mut old = vec![];
        let mut own_recent = vec![];
        let mut own_old = vec![];
        for MRWithVersions { mr, versions } in &mrs {
            if mr.author.username == me {
                let too_old = chrono::Utc::now() - mr.updated_at > chrono::Duration::weeks(13);
                let too_many = own_recent.len() >= 10;
                if too_old || too_many {
                    own_old.push(mr);
                } else {
                    own_recent.push(mr);
                }
                continue;
            }
            let mut f = || {
                let (_, latest_rev) = versions
                    .last_key_value()
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
                let partially_reviewed = versions
                    .iter()
                    .flat_map(|(_, ver)| version_stats(repo, ver))
                    .any(|stats| stats[Status::Reviewed] > 0);
                let is_interesting = assigned || watchlist_hit || partially_reviewed;

                if is_interesting {
                    interesting.push((mr, n_unreviewed));
                } else {
                    let too_old = chrono::Utc::now() - mr.updated_at > chrono::Duration::weeks(5);
                    let too_many = recent.len() >= 10;
                    if too_old || too_many {
                        old.push(mr);
                    } else if mr.draft {
                        drafts.push(mr);
                    } else {
                        recent.push(mr);
                    }
                }
                anyhow::Ok(())
            };
            match f() {
                Ok(()) => (),
                Err(e) => {
                    error!("{}: {}", mr.iid.0, e);
                    continue;
                }
            }
        }

        if !interesting.is_empty() {
            println!("Relevant merge requests:");
            println!();
        }
        let mut tw = TabWriter::new(std::io::stdout()).ansi(true);
        for (mr, n_unreviewed) in &interesting {
            let when = timeago::Formatter::new().convert_chrono(mr.updated_at, chrono::Utc::now());
            writeln!(
                tw,
                "  {}{}\t{}\t{}\t{}\t({} left to review)",
                Paint::yellow("!").bold(),
                Paint::yellow(mr.iid.0).bold(),
                Paint::blue(&when).bold(),
                Paint::green(&mr.author.username).bold(),
                Paint::new(&mr.title).bold(),
                Paint::new(n_unreviewed),
            )?;
        }
        tw.flush()?;
        if !interesting.is_empty() {
            println!();
        }

        if !recent.is_empty() {
            println!("New merge requests:");
            println!();
        }
        let mut tw = TabWriter::new(std::io::stdout()).ansi(true);
        for mr in &recent {
            let when = timeago::Formatter::new().convert_chrono(mr.updated_at, chrono::Utc::now());
            writeln!(
                tw,
                "  {}{}\t{}\t{}\t{}\t",
                Paint::yellow("!"),
                Paint::yellow(mr.iid.0),
                Paint::blue(&when),
                Paint::green(&mr.author.username).italic(),
                &mr.title,
            )?;
        }
        tw.flush()?;
        if !recent.is_empty() {
            println!();
        }

        if !old.is_empty() {
            println!("...and {} more (use \"orpa mrs\" to see them)", old.len());
            println!();
        }

        if !drafts.is_empty() {
            println!(
                "({} were hidden because they're marked as drafts)",
                drafts.len()
            );
            println!();
        }

        if !own_recent.is_empty() {
            println!("Your own MRs:");
            println!();
        }
        let mut tw = TabWriter::new(std::io::stdout()).ansi(true);
        for mr in &own_recent {
            let when = timeago::Formatter::new().convert_chrono(mr.updated_at, chrono::Utc::now());
            writeln!(
                tw,
                "  {}{}\t{}\t{}\t{}\t",
                Paint::yellow("!"),
                Paint::yellow(mr.iid.0),
                Paint::blue(&when),
                Paint::green(&mr.author.username).italic(),
                &mr.title,
            )?;
        }
        tw.flush()?;
        if !own_recent.is_empty() {
            println!();
        }

        if !own_old.is_empty() {
            println!(
                "...and {} more (use \"orpa mrs\" to see them)",
                own_old.len()
            );
            println!();
        }

        if !interesting.is_empty() || !recent.is_empty() || !own_recent.is_empty() {
            println!("Use \"orpa mr <id>\" to see the full MR information");
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
            project_id: ProjectId(config.get_i64("gitlab.projectId")? as u64),
            token: config.get_string("gitlab.privateToken")?,
        })
    }
}

fn db_path(repo: &Repository) -> PathBuf {
    OPTS.db.clone().unwrap_or_else(|| repo.path().join("orpa"))
}

fn cached_mrs(repo: &Repository) -> anyhow::Result<Vec<MRWithVersions>> {
    let mr_dir = db_path(repo).join("merge_requests");
    let mut mrs = vec![];
    for entry in std::fs::read_dir(mr_dir)? {
        let mr: MRWithVersions = serde_json::from_reader(File::open(entry?.path())?)?;
        mrs.push(mr);
    }
    mrs.sort_by_key(|mr| std::cmp::Reverse(mr.mr.updated_at));
    Ok(mrs)
}

fn merge_request(repo: &Repository, target: String) -> anyhow::Result<()> {
    pager::Pager::with_pager("less -FRSX").setup();
    let target = target.trim_matches(|c: char| !c.is_numeric());
    let path = db_path(repo).join("merge_requests").join(target);
    let MRWithVersions { mr, versions } = serde_json::from_reader(File::open(path)?)?;

    let config = repo.config()?;
    let me = config.get_string("gitlab.username")?;
    print_mr(&me, &mr);
    println!();
    for version in versions.values() {
        print_version(repo, version)?;
    }
    println!();
    if let Some((_, version)) = versions.last_key_value() {
        if let Ok((base, head)) = resolve_version(repo, version) {
            let diff = repo.diff_tree_to_tree(Some(&base.tree()?), Some(&head.tree()?), None)?;
            print_diff_stat(diff)?;
            println!();
        }

        let range = format!("{}..{}", &version.base.0, &version.head.0);
        let mut walk = repo.revwalk()?;
        walk.push_range(&range)?;
        walk.set_sorting(git2::Sort::REVERSE)?;
        for oid in walk {
            let commit = repo.find_commit(oid?)?;
            print_commit(commit);
        }
    }
    Ok(())
}

fn print_commit(commit: Commit) {
    println!("{}{}", Paint::yellow("commit "), Paint::yellow(commit.id()));
    if let Some((name, email)) = commit.author().name().zip(commit.author().email()) {
        println!("Author: {} <{}>", name, email);
    }
    let date = git_time_to_chrono(commit.time());
    println!("Date:   {}", date);
    println!();
    if let Some(msg) = commit.message() {
        for line in textwrap::wrap(msg, 96) {
            println!("    {}", line);
        }
    }
}

fn git_time_to_chrono(time: git2::Time) -> chrono::DateTime<chrono::FixedOffset> {
    let tz = chrono::FixedOffset::east_opt(time.offset_minutes() * 60).unwrap();
    let date = chrono::DateTime::from_timestamp(time.seconds(), 0).unwrap();
    date.with_timezone(&tz)
}

fn merge_requests(repo: &Repository, include_all: bool) -> anyhow::Result<()> {
    pager::Pager::with_pager("less -FRSX").setup();
    let config = repo.config()?;
    let me = config.get_string("gitlab.username")?;
    let mut mrs = cached_mrs(repo)?;
    mrs.retain(|mr| include_all || (!mr.mr.draft && mr.mr.author.username != me));
    for MRWithVersions { mr, versions } in mrs {
        print_mr(&me, &mr);
        println!();
        for version in versions.values() {
            print_version(repo, version)?;
        }
        println!();
        if let Some((base, head)) = versions
            .last_key_value()
            .and_then(|(_, v)| resolve_version(repo, v).ok())
        {
            let diff = repo.diff_tree_to_tree(Some(&base.tree()?), Some(&head.tree()?), None)?;
            print_diff_stat(diff)?;
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

fn resolve_version<'repo>(
    repo: &'repo Repository,
    version: &VersionInfo,
) -> anyhow::Result<(Commit<'repo>, Commit<'repo>)> {
    Ok(repo
        .find_commit(version.base.as_oid())
        .and_then(|x| repo.find_commit(version.head.as_oid()).map(|y| (x, y)))?)
}

fn print_version(repo: &Repository, version: &VersionInfo) -> anyhow::Result<()> {
    let (base, head) = match resolve_version(repo, version) {
        Ok(x) => x,
        Err(_) => {
            let base = &version.base.0[..7];
            let head = &version.head.0[..7];
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

fn count_reviewed(repo: &Repository, version: &VersionInfo) -> anyhow::Result<(usize, usize)> {
    let range = format!("{}..{}", &version.base.0, &version.head.0);
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
        Paint::yellow(mr.iid.0),
        mr.source_branch,
        mr.target_branch,
    );
    println!("Status: {}", fmt_state(mr.state));
    println!("Author: {} (@{})", &mr.author.name, &mr.author.username);
    println!("Date:   {}", &mr.updated_at);
    println!();
    println!("    {}", &mr.title);

    if let Some(desc) = mr.description.as_ref().filter(|x| !x.is_empty()) {
        println!();
        for line in textwrap::wrap(desc, 96) {
            println!("    {}", line);
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
fn mr_paths(repo: &Repository, mr: &VersionInfo) -> anyhow::Result<Vec<PathBuf>> {
    let base = repo.find_commit(mr.base.as_oid())?.tree()?;
    let head = repo.find_commit(mr.head.as_oid())?.tree()?;
    let diff = repo.diff_tree_to_tree(Some(&base), Some(&head), None)?;
    let mut paths = HashSet::<&Path>::default();
    for delta in diff.deltas() {
        if let Some(path) = delta.new_file().path() {
            paths.insert(path);
        }
    }
    Ok(paths.into_iter().map(|x| x.to_path_buf()).collect())
}
