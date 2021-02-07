use atoi::atoi;
use gitlab::{Gitlab, MergeRequest, MergeRequestStateFilter, ProjectId};
use serde::*;
use structopt::StructOpt;
use tracing::*;

#[derive(StructOpt)]
struct Opts {
    #[structopt(long)]
    config: Option<std::path::PathBuf>,
    #[structopt(long)]
    db: Option<std::path::PathBuf>,
}

#[derive(Deserialize)]
struct Config {
    gitlab_host: String,
    gitlab_token: String,
    project_id: ProjectId,
    me: String,
}

fn main() -> anyhow::Result<()> {
    let opts = Opts::from_args();
    tracing_subscriber::fmt::init();
    let dirs = directories::ProjectDirs::from("com", "asayers", "incoming").unwrap();

    info!("Loading the config");
    let config_path = opts
        .config
        .unwrap_or_else(|| dirs.config_dir().join("config.toml"));
    let config: Config = toml::from_slice(&std::fs::read(config_path)?)?;

    info!("Opening the database");
    let db_path = opts.db.unwrap_or_else(|| dirs.data_dir().to_path_buf());
    let db = sled::open(db_path)?;

    info!("Connecting to gitlab at {}", &config.gitlab_host);
    let gl = Gitlab::new_insecure(&config.gitlab_host, &config.gitlab_token).unwrap();

    info!("Fetching all open MRs for project {}", config.project_id);
    let mrs = gl.merge_requests_with_state(config.project_id, MergeRequestStateFilter::Opened)?;
    for mr in mrs {
        let assigned_to_me = mr
            .assignees
            .iter()
            .flatten()
            .any(|x| x.username == config.me);
        println!(
            "!{}{}: {} [{}]",
            mr.iid.value(),
            if assigned_to_me { "*" } else { "" },
            mr.title,
            mr.author.username,
        );
        let base = mr_base(&mr);
        let head = mr.sha.as_ref().map_or("", |x| x.value());
        let current_range = format!("{}..{}", base, head);

        let prefix = format!("{:06}#", mr.iid.value());
        let existing = db.scan_prefix(prefix.as_bytes());
        let mut latest_rev = None;
        let mut latest_range = None;
        for x in existing {
            let (k, v) = x?;
            let rev: u16 = atoi(&k[7..]).unwrap();
            let range = String::from_utf8(v.to_vec())?;
            println!("  #{}: {}", rev, range);
            latest_rev = Some(rev);
            latest_range = Some(range);
        }

        if latest_range.as_ref() != Some(&current_range) {
            info!("Inserting new revision!");
            let new_rev = latest_rev.map_or(0, |x| x + 1);
            let key = format!("{:06}#{:04}", mr.iid.value(), new_rev);
            db.insert(key.as_bytes(), current_range.as_bytes())?;
            println!("  #{}: {}", new_rev, current_range);
        }
    }
    Ok(())
}

fn mr_base(mr: &MergeRequest) -> &str {
    if let Some(x) = mr
        .diff_refs
        .as_ref()
        .and_then(|x| x.base_sha.as_ref())
        .map(|x| x.value())
    {
        // They told is the base; good - use that.
        return x;
    }
    ""
}
