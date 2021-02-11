use git2::Repository;
use gitlab::{Gitlab, MergeRequest, MergeRequestStateFilter, ProjectId};
use mr_db::RevInfo;
use std::fs::File;
use structopt::StructOpt;
use tracing::*;
use yansi::Paint;

#[derive(StructOpt)]
/// A local database of gitlab MR revisions
///
/// The user's own MRs are hidden by default, as are WIP MRs.
struct Opts {
    #[structopt(long)]
    db: Option<std::path::PathBuf>,
    /// Include hidden MRs.
    #[structopt(long, short)]
    hidden: bool,
    /// Sync MRs from gitlab.
    #[structopt(long, short)]
    fetch: bool,
}

fn main() -> anyhow::Result<()> {
    let opts = Opts::from_args();
    tracing_subscriber::fmt::init();

    info!("Opening the git repo");
    let repo = Repository::open_from_env()?;

    info!("Loading the config");
    let config = repo.config()?;
    let gitlab_host = config.get_string("gitlab.url")?;
    let gitlab_token = config.get_string("gitlab.privateToken")?;
    let project_id = ProjectId::new(config.get_i64("gitlab.projectId")? as u64);
    let me = config.get_string("gitlab.username")?;

    info!("Opening the database");
    let default_path = repo.path().join("merge_requests");
    let db_path = opts.db.as_ref().unwrap_or_else(|| &default_path);
    let db = mr_db::Db::open(&db_path)?;

    let mr_cache_path = db_path.join("mr_cache");
    let mrs = if opts.fetch {
        info!("Connecting to gitlab at {}", &gitlab_host);
        let gl = Gitlab::new_insecure(&gitlab_host, &gitlab_token).unwrap();

        info!("Fetching all open MRs for project {}", project_id);
        let mrs = gl.merge_requests_with_state(project_id, MergeRequestStateFilter::Opened)?;
        serde_json::to_writer(File::create(mr_cache_path)?, &mrs)?;

        info!("Updating the DB with new revisions");
        for mr in &mrs {
            db.insert_if_newer(&repo, &gl, project_id, mr)?;
        }
        mrs
    } else {
        info!("Reading cached MRs from {}", mr_cache_path.display());
        serde_json::from_reader(File::open(mr_cache_path)?)?
    };

    info!("Printing MR info");
    for mr in mrs
        .iter()
        .filter(|mr| opts.hidden || (!mr.work_in_progress && mr.author.username != me))
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
