use atoi::atoi;
use git2::{Oid, Repository};
use gitlab::{Gitlab, MergeRequest, MergeRequestStateFilter, ProjectId};
use structopt::StructOpt;
use tracing::*;

#[derive(StructOpt)]
struct Opts {
    #[structopt(long)]
    db: Option<std::path::PathBuf>,
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
    let db_path = opts.db.unwrap_or_else(|| repo.path().join("gitlab_mrs"));
    let db = sled::open(db_path)?;

    info!("Connecting to gitlab at {}", &gitlab_host);
    let gl = Gitlab::new_insecure(&gitlab_host, &gitlab_token).unwrap();

    info!("Fetching all open MRs for project {}", project_id);
    let mrs = gl.merge_requests_with_state(project_id, MergeRequestStateFilter::Opened)?;
    for mr in mrs {
        let assigned_to_me = mr.assignees.iter().flatten().any(|x| x.username == me);
        println!(
            "!{}{}: {} [{}]",
            mr.iid.value(),
            if assigned_to_me { "*" } else { "" },
            mr.title,
            mr.author.username,
        );

        let prefix = format!("{:06}#", mr.iid.value());
        let existing = db.scan_prefix(prefix.as_bytes());
        let mut latest = None;
        for x in existing {
            let (k, v) = x?;
            let rev: u16 = atoi(&k[7..]).unwrap();
            let base = Oid::from_bytes(&v[..20])?;
            let head = Oid::from_bytes(&v[20..])?;
            println!("  #{}: {}..{}", rev, base, head);
            latest = Some((rev, base, head));
        }

        // We only update the DB if the head has changed.  Technically we
        // should re-check the base each time as well (in case the target
        // branch has changed); however, this means making an API request
        // per-MR, and is slow.
        let current_head = Oid::from_str(mr.sha.as_ref().unwrap().value())?;
        if latest.map(|(_, _, head)| head) != Some(current_head) {
            let current_base = mr_base(&repo, &gl, project_id, &mr, current_head)?;
            let current_range = format!("{}..{}", current_base, current_head);
            info!("Inserting new revision!");
            let new_rev = latest.map_or(0, |(x, _, _)| x + 1);
            let key = format!("{:06}#{:04}", mr.iid.value(), new_rev);
            let mut val = Box::new([0; 40]);
            val[..20].copy_from_slice(current_base.as_bytes());
            val[20..].copy_from_slice(current_head.as_bytes());
            db.insert(key.as_bytes(), val as Box<[u8]>)?;
            println!("  #{}: {}", new_rev, current_range);
        }
    }
    Ok(())
}

fn mr_base<'a>(
    repo: &'a Repository,
    gl: &'a Gitlab,
    project_id: ProjectId,
    mr: &'a MergeRequest,
    head: Oid,
) -> anyhow::Result<Oid> {
    if let Some(x) = mr.diff_refs.as_ref().and_then(|x| x.base_sha.as_ref()) {
        // They told us the base; good - use that.
        Ok(Oid::from_str(x.value())?)
    } else {
        // Looks like we're gonna have to work it out ourselves...
        let params: [(String, String); 0] = [];
        // Get the target SHA directly from gitlab, in case the local repo
        // is out-of-date.
        let target = gl
            .branch(project_id, &mr.target_branch, &params)?
            .commit
            .unwrap();
        let target_oid = Oid::from_str(target.id.value())?;
        Ok(repo.merge_base(head, target_oid)?)
    }
}
