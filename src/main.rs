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
use orpa::*;
use std::env;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
struct Args {
    /// Path to the file describing the rules
    #[structopt(short = "m", long = "maintainers", default_value = "MAINTAINERS")]
    maintainers: String,
    /// Path to the approvals DB
    #[structopt(short = "a", long = "approvals", default_value = ".approvals")]
    approvals: String,
    #[structopt(subcommand)]
    subcommand: Subcommand,
}

#[derive(StructOpt, Debug)]
enum Subcommand {
    /// Show the rules which match a given file
    #[structopt(name = "rules")]
    Rules { target: String },
    /// Show the approvals for a given file
    #[structopt(name = "approvals")]
    Approvals { pathspec: String },
    /// Show the unmet requirements for a given commit
    #[structopt(name = "status")]
    Status { commitspec: Option<String> },
    /// Approve a file
    #[structopt(name = "approve")]
    Approve {
        #[structopt(short = "l", parse(from_occurrences))]
        lvl: usize,
        targets: Vec<String>,
    },
}

impl Args {
    fn load_ruleset(&self) -> RuleSet {
        let maintainers_file = File::open(&self.maintainers).unwrap();
        RuleSet::from_reader(maintainers_file).unwrap()
    }
    fn load_approvals(&self) -> Approvals {
        let approvals_file = File::open(&self.approvals).unwrap();
        Approvals::from_reader(approvals_file).unwrap()
    }
}

fn main() {
    env_logger::init();

    let args = Args::from_args();
    info!("Args: {:?}", args);

    match &args.subcommand {
        Subcommand::Rules { target } => {
            let ruleset = args.load_ruleset();
            let rules = ruleset.matching(&Path::new(target));
            print!("{}", rules);
        }
        Subcommand::Approvals { pathspec } => {
            let repo = Repository::open_from_env().unwrap();
            let oid = repo.revparse_single(pathspec).unwrap().id();
            let approvals = args.load_approvals();
            let aps = approvals.lookup(oid);
            for (name, lvl) in aps {
                println!("{}\t{}", name, lvl);
            }
        }
        Subcommand::Status { commitspec } => {
            let repo = Repository::open_from_env().unwrap();
            let ruleset = args.load_ruleset();
            let approvals = args.load_approvals();
            let commitspec = commitspec.as_ref().map(|x| x.as_str()).unwrap_or("HEAD");
            let clean = status(repo, ruleset, approvals, commitspec);
            if !clean {
                process::exit(1);
            }
        }
        Subcommand::Approve { targets, lvl } => {
            let repo = Repository::open_from_env().unwrap();
            let mut approvals_file = OpenOptions::new()
                .append(true)
                .open(args.approvals)
                .unwrap();
            for target in targets {
                let mut pathspec = String::from("HEAD:");
                pathspec.push_str(&target);
                let oid = repo.revparse_single(&pathspec).unwrap().id();
                let name = env::var("USER").unwrap();
                let lvl = Level(*lvl);
                writeln!(approvals_file, "{}\t{}\t{}", oid, name, lvl).unwrap();
            }
        }
    }
}

fn status(repo: Repository, ruleset: RuleSet, approvals: Approvals, commitspec: &str) -> bool {
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
                    if clean {
                        println!("The following requirements are unmet:");
                    }
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
