#[macro_use]
extern crate log;
extern crate env_logger;
extern crate glob;
extern crate itertools;
extern crate orpa;
#[macro_use]
extern crate structopt;
extern crate git2;

use git2::*;
use itertools::Itertools;
use orpa::*;
use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::path::Path;
use std::path::PathBuf;
use std::process::{self, Command};
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
struct Args {
    /// Path to the file describing the rules
    #[structopt(short = "m", long = "maintainers", default_value = "MAINTAINERS")]
    maintainers: String,
    /// Increase verbosity
    #[structopt(short = "v", parse(from_occurrences))]
    verbosity: usize,
    #[structopt(subcommand)]
    subcommand: Subcommand,
}

#[derive(StructOpt, Debug)]
enum Subcommand {
    /// Show the rules which match a given file
    #[structopt(name = "rules")]
    Rules { target: String },

    /// Show the approvals for a given file.  Can be a reference to a blob, or a path (in which
    /// case HEAD is assumed).
    #[structopt(name = "approvals")]
    Approvals { pathspec: String },

    /// Show the unmet requirements for a given commit
    #[structopt(name = "status")]
    Status {
        /// The commit to display the status of
        #[structopt(default_value = "HEAD")]
        commitspec: String,
    },

    /// Approve a file
    #[structopt(name = "approve")]
    Approve {
        /// Increase the scrutiny level.  Can be specified multiple times.
        #[structopt(short = "l", parse(from_occurrences))]
        lvl: usize,
        /// The files to approve.  Can be a reference to a blob, or a path (in which case HEAD is
        /// assumed).
        targets: Vec<String>,
    },

    /// Sync approvals with the given remote
    #[structopt(name = "sync")]
    SyncC {
        #[structopt(default_value = "origin")]
        remote: String,
    },

    ///
    #[structopt(name = "todo")]
    ToDo {
        /// The commit to display the to-dos for
        #[structopt(default_value = "HEAD")]
        commitspec: String,
    },
}

impl Args {
    fn load_ruleset(&self) -> RuleSet {
        let maintainers_file = File::open(&self.maintainers).unwrap();
        RuleSet::from_reader(maintainers_file).unwrap()
    }
}

fn main() {
    let args = Args::from_args();

    let log_level = match args.verbosity {
        0 => log::LevelFilter::Warn,
        1 => log::LevelFilter::Info,
        _ => log::LevelFilter::Trace,
    };
    env_logger::Builder::from_default_env()
        .filter_level(log_level)
        .init();

    info!("Args: {:?}", args);

    match &args.subcommand {
        Subcommand::Rules { target } => {
            let ruleset = args.load_ruleset();
            let rules = ruleset.matching(&Path::new(target));
            print!("{}", rules);
        }
        Subcommand::Approvals { pathspec } => {
            let repo = Repository::open_from_env().unwrap();
            let oid = parse_pathspec(&repo, pathspec).unwrap();
            let aps = approvals(&repo, oid).unwrap();
            for (name, lvl) in aps {
                println!("{}\t{}", name, lvl);
            }
        }
        Subcommand::Status { commitspec } => {
            let repo = Repository::open_from_env().unwrap();
            let ruleset = args.load_ruleset();
            let exit_code = status(&repo, ruleset, &commitspec).unwrap();
            process::exit(exit_code);
        }
        Subcommand::ToDo { commitspec } => {
            let name = env::var("USER").unwrap();
            let repo = Repository::open_from_env().unwrap();
            let ruleset = args.load_ruleset();
            let reqs = status_2(&repo, ruleset, &commitspec).unwrap();
            for (path, mut rules) in reqs {
                rules.retain(|rule| rule.pop.contains(&name));
                if !rules.is_empty() {
                    println!("{}:", path.to_str().unwrap());
                    for rule in rules {
                        println!("  {}", rule);
                    }
                }
            }
        }
        Subcommand::Approve { targets, lvl } => {
            let repo = Repository::open_from_env().unwrap();
            for target in targets {
                let oid = parse_pathspec(&repo, target).unwrap();
                let name = env::var("USER").unwrap();
                let lvl = Level(*lvl);
                approve(&repo, oid, name, lvl).unwrap();
            }
        }
        Subcommand::SyncC { remote } => {
            let repo = Repository::open_from_env().unwrap();
            info!("Connecting to {}", remote);
            let mut remote = repo.find_remote(&remote).unwrap();
            sync(remote);
        }
    }
}

const NOTES_REF: &str = "refs/notes/orpa";
const NOTES_TMP_REF: &str = "refs/notes/orpa-tmp";

fn sync(mut remote: Remote) {
    info!("Fetching...");
    let mut opts = FetchOptions::new();
    remote
        .fetch(
            &[&format!("{}:{}", NOTES_REF, NOTES_TMP_REF)],
            Some(opts.remote_callbacks(mk_cbs())),
            None,
        )
        .unwrap();
    info!("Merging...");
    let merge_cmd = Command::new("git")
        .arg("notes")
        .arg(&format!("--ref={}", NOTES_REF))
        .arg("merge")
        .arg("--strategy=cat_sort_uniq")
        .arg(NOTES_TMP_REF).status();
    assert!(merge_cmd.unwrap().success());
    info!("Pushing...");
    let mut opts = PushOptions::new();
    remote
        .push(
            &[&format!("{}:{}", NOTES_REF, NOTES_REF)],
            Some(opts.remote_callbacks(mk_cbs())),
        )
        .unwrap();
}

fn mk_cbs<'a>() -> RemoteCallbacks<'a> {
    let mut cbs = RemoteCallbacks::new();
    cbs.credentials(|url, username, cred_type| {
        info!("Attempting to authenticate for {}", url);
        if cred_type.is_ssh_key() {
            Cred::ssh_key_from_agent(username.unwrap())
        } else {
            let msg = format!(
                "Don't know how to authenticate request of type {:?}",
                cred_type
            );
            Err(git2::Error::from_str(&msg))
        }
    });
    cbs.push_update_reference(|r, status| {
        if let Some(e) = status {
            error!("Pushing {} rejected: {}", r, e);
        }
        Ok(())
    });
    cbs
}

fn parse_pathspec(repo: &Repository, pathspec: &str) -> Result<Oid> {
    Ok(repo
        .revparse_single(pathspec)
        .unwrap_or_else(|_| {
            let mut pathspec2 = String::from("HEAD:");
            pathspec2.push_str(pathspec);
            warn!("Interpreting as {}", pathspec2);
            repo.revparse_single(&pathspec2).unwrap()
        })
        .id())
}

fn approvals(repo: &Repository, target: Oid) -> Result<Vec<(Name, Level)>> {
    Ok(match repo.find_note(Some(NOTES_REF), target) {
        Ok(note) => note
            .message()
            .unwrap()
            .lines()
            .map(|l| {
                let mut fields = l.split_whitespace();
                let name = fields.next().unwrap().to_string();
                let lvl: Level = fields.next().unwrap().parse().unwrap();
                (name, lvl)
            })
            .collect(),
        Err(_) => Vec::new(),
    })
}

fn approve(repo: &Repository, target: Oid, name: Name, lvl: Level) -> Result<()> {
    let cfg = repo.config()?.snapshot()?;
    let sig = Signature::now(cfg.get_str("user.name")?, cfg.get_str("user.email")?)?;
    let existing = repo.find_note(Some(NOTES_REF), target);
    let mut lines: Vec<String> = match existing {
        Ok(note) => note
            .message()
            .unwrap()
            .lines()
            .map(|x| x.to_string())
            .collect(),
        Err(_) => vec![],
    };
    lines.push(format!("{}\t{}", name, lvl));
    lines.sort();
    lines.dedup();
    let msg: String = lines.into_iter().intersperse("\n".to_string()).collect();
    repo.note(&sig, &sig, Some(NOTES_REF), target, &msg, true)?;
    info!("Approved {:?}: {}", target, msg);
    Ok(())
}

fn status_2(
    repo: &Repository,
    ruleset: RuleSet,
    commitspec: &str,
) -> Result<HashMap<PathBuf, Vec<Rule>>> {
    let oid = repo.revparse_single(&commitspec)?.id();
    let commit = repo.find_commit(oid)?;
    let tree = commit.tree()?;
    let mut reqs = HashMap::new();
    tree.walk(TreeWalkMode::PreOrder, |prefix, entry| {
        match entry.kind() {
            Some(ObjectType::Blob) => {
                let path = PathBuf::from(prefix).join(entry.name().unwrap());
                let mut rules = ruleset.matching(&path);
                let oid = entry.id();
                let aps = approvals(&repo, oid).unwrap();
                for (name, lvl) in aps {
                    rules.approve(&name, lvl);
                }
                for rule in rules.0 {
                    reqs.entry(path.clone()).or_insert(Vec::new()).push(rule);
                }
            }
            _ => {}
        }
        0
    })?;
    Ok(reqs)
}

fn status(repo: &Repository, ruleset: RuleSet, commitspec: &str) -> Result<i32> {
    let reqs = status_2(repo, ruleset, commitspec)?;
    if reqs.is_empty() {
        println!("All changes approved");
        Ok(0)
    } else {
        println!("The following requirements are unmet:");
        for (path, rules) in reqs {
            println!("{}:", path.to_str().unwrap());
            for rule in rules {
                println!("  {}", rule);
            }
        }
        Ok(1)
    }
}
