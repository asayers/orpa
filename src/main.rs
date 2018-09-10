#[macro_use]
extern crate log;
extern crate env_logger;
extern crate glob;
extern crate orpa;
extern crate itertools;
#[macro_use]
extern crate structopt;
extern crate git2;

use std::process;
use orpa::*;
use std::fs::File;
use git2::*;
use std::path::Path;
use std::path::PathBuf;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
struct Args {
    #[structopt(short = "m", long = "maintainers")]
    maintainers: Option<String>,
    #[structopt(short = "a", long = "approvals")]
    approvals: Option<String>,
    #[structopt(subcommand)]
    subcommand: Subcommand,
}

#[derive(StructOpt, Debug)]
enum Subcommand {
    #[structopt(name = "rules")]
    Rules {
        target: String,
    },
    #[structopt(name = "approvals")]
    Approvals {
        pathspec: String,
    },
    #[structopt(name = "status")]
    Status {
        commitspec: Option<String>,
    },
}

impl Args {
    fn maintainers_path(&self) -> &str {
        self.maintainers
            .as_ref()
            .map(|x| x.as_str())
            .unwrap_or("MAINTAINERS")
    }
    fn approvals_path(&self) -> &str {
        self.approvals
            .as_ref()
            .map(|x| x.as_str())
            .unwrap_or(".approvals")
    }
}

fn main() {
    env_logger::init();

    let args = Args::from_args();
    info!("Args: {:?}", args);

    let maintainers_file = File::open(args.maintainers_path()).unwrap();
    let approvals_file = File::open(args.approvals_path()).unwrap();
    let ruleset = RuleSet::from_reader(maintainers_file).unwrap();
    let approvals = Approvals::from_reader(approvals_file).unwrap();

    let repo = Repository::open_from_env().unwrap();

    match args.subcommand {
        Subcommand::Rules { target } => {
            let rules = ruleset.matching(&Path::new(&target));
            print!("{}", rules);
        }
        Subcommand::Approvals { pathspec } => {
            let oid = repo.revparse_single(&pathspec).unwrap().id();
            let aps = approvals.lookup(oid);
            for (name, lvl) in aps {
                println!("{}\t{}", name, lvl);
            }
        }
        Subcommand::Status { commitspec } => {
            let commitspec = commitspec.unwrap_or("HEAD".to_string());
            let clean = status(repo, ruleset, approvals, commitspec);
            if !clean {
                process::exit(1);
            }
        }
    }
}

fn status(repo: Repository, ruleset: RuleSet, approvals: Approvals, commitspec: String) -> bool {
    let oid = repo.revparse_single(&commitspec).unwrap().id();
    let commit = repo.find_commit(oid).unwrap();
    let tree = commit.tree().unwrap();
    let mut clean = true;
    tree.walk(TreeWalkMode::PreOrder, |prefix, entry| {
        match entry.kind() {
            Some(ObjectType::Blob) => {
                let path = PathBuf::from(prefix).join(entry.name().unwrap());
                let mut rules = ruleset.matching(&path);
                let oid = entry.id();
                let aps = approvals.lookup(oid);
                for (name, lvl) in aps {
                    rules.approve(name, *lvl);
                }
                if !rules.0.is_empty() {
                    println!("{}:", path.to_str().unwrap());
                    for rule in rules.0 {
                        println!("  {}", rule);
                    }
                    clean = false;
                }
            }
            _ => {}
        }
        0
    }).unwrap();
    if clean {
        println!("All changes approved");
    }
    clean
}
