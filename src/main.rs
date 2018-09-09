#[macro_use]
extern crate failure;
#[macro_use]
extern crate log;
extern crate env_logger;
extern crate glob;
extern crate itertools;
#[macro_use]
extern crate structopt;

mod reqs;
mod rules;

use rules::*;
use std::collections::HashSet;
use std::fs::File;
use std::iter::FromIterator;
use std::path::Path;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
struct Args {
    targets: Vec<String>,
    #[structopt(short = "m", long = "maintainers")]
    maintainers: Option<String>,
}

impl Args {
    fn maintainers_path(&self) -> &str {
        self.maintainers
            .as_ref()
            .map(|x| x.as_str())
            .unwrap_or("MAINTAINERS")
    }
}

fn main() {
    env_logger::init();

    let args = Args::from_args();
    info!("Args: {:?}", args);

    let maintainers_file = File::open(args.maintainers_path()).unwrap();
    let ruleset = RuleSet::from_reader(maintainers_file).unwrap();

    for target in &args.targets {
        let mut reqs = ruleset.reqs_for(&Path::new(target));
        info!("Rules: {:?}", reqs);

        let approvals: HashSet<(String, Scrutiny)> = HashSet::from_iter(vec![]);
        info!("Approvals: {:?}", approvals);

        for (name, lvl) in approvals {
            reqs.approve(name, lvl);
        }

        reqs.normalize();
        if !reqs.is_satisfied() {
            println!("{}: {:?}", target, reqs)
        }
    }
}
