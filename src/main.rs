extern crate failure;
extern crate glob;
extern crate itertools;
#[macro_use]
extern crate structopt;

mod rules;

use rules::*;
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::iter::FromIterator;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
struct Args {
    target: String,
    maintainers: Option<String>,
}

fn main() {
    let args = Args::from_args();
    eprintln!("Args: {:?}", args);

    let maintainers = args
        .maintainers
        .as_ref()
        .map(|x| x.as_str())
        .unwrap_or("MAINTAINERS");
    let file = BufReader::new(File::open(maintainers).unwrap());
    let rules: Vec<_> = file
        .lines()
        .map(|l| l.unwrap())
        .map(|mut l| {
            if let Some(i) = l.find('#') {
                l.truncate(i);
            }
            l
        })
        .filter(|l| !l.is_empty())
        .map(|l| l.parse::<Rule>().unwrap())
        .filter(|rule| rule.matches(&args.target))
        .collect();
    eprintln!("Matching rules: {:?}", rules);

    let approvals = HashSet::from_iter(vec![]);
    eprintln!("Approvals: {:?}", approvals);

    let dnf = DNF::from_iter(rules);
    if dnf.is_empty() {
        eprintln!("WARN: Unsatisfiable rules!");
    }
    let mut fixes = dnf.fixes(approvals);
    fixes.minimize();
    print!("{}", fixes);
}
