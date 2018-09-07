extern crate failure;
extern crate glob;
extern crate itertools;
#[macro_use]
extern crate structopt;

use itertools::Itertools;
use std::collections::HashSet;
use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::iter::FromIterator;
use std::str::FromStr;
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
#[structopt(name = "check_approval")]
struct Args {
    target: String,
    maintainers: Option<String>,
}

fn main() {
    let args = Args::from_args();
    println!("Args: {:?}", args);

    let maintainers = args
        .maintainers
        .as_ref()
        .map(|x| x.as_str())
        .unwrap_or("MAINTAINERS");
    let file = BufReader::new(File::open(maintainers).unwrap());
    let rules: Vec<_> = file
        .lines()
        .map(|l| l.unwrap().parse::<Rule>().unwrap())
        .filter(|rule| rule.pat.matches(&args.target))
        .collect();
    println!("Rules: {:?}", rules);

    let approvals = HashSet::from_iter(vec![]);
    println!("Approvals: {:?}", approvals);

    let mut fixes = DNF::from_iter(rules).fixes(approvals);
    fixes.minimize();
    print!("{}", fixes);
}

/// A rule is satisfied when any `n` members of `pop` approve.
#[derive(Debug)]
struct Rule {
    pat: glob::Pattern,
    pop: Vec<String>,
    n: usize,
}

impl FromStr for Rule {
    type Err = failure::Error;

    fn from_str(line: &str) -> Result<Rule, failure::Error> {
        let mut ws = line.split_whitespace();
        let pat = glob::Pattern::new(ws.next().unwrap())?;
        let n: usize = ws.next().unwrap().parse()?;
        let pop = ws
            .next()
            .unwrap()
            .split(',')
            .map(|x| x.to_owned())
            .collect();
        Ok(Rule { pat, n, pop })
    }
}

/// Sets of approvers in disjunct normal form.
///
/// * A rule can be represented in DNF; it lists the sets of approvers which would satisfy the
///   rule.
/// * A set of rules can be represented in DNF; it list of sets of approvers which would satisfy
///   all the rules.
/// * The potential "fixes" to the current set of approvers is also represented in DNF.
///
#[derive(Clone, Debug)]
struct DNF(Vec<HashSet<String>>);

impl fmt::Display for DNF {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for set in &self.0 {
            writeln!(f, "{:?}", set)?;
        }
        Ok(())
    }
}

impl IntoIterator for DNF {
    type Item = HashSet<String>;
    type IntoIter = std::vec::IntoIter<HashSet<String>>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl From<Rule> for DNF {
    /// Represent a rule in DNF.
    ///
    /// eg. 2 of { A, B, C } => { A, B } or { A, C } or { B, C }
    fn from(rule: Rule) -> DNF {
        DNF(rule
            .pop
            .into_iter()
            .combinations(rule.n)
            .map(|x| HashSet::from_iter(x))
            .collect())
    }
}

impl FromIterator<Rule> for DNF {
    /// Represent a set of rules in DNF.
    fn from_iter<I: IntoIterator<Item = Rule>>(iter: I) -> DNF {
        iter.into_iter().map(|rule| DNF::from(rule)).collect()
    }
}

impl FromIterator<DNF> for DNF {
    /// Merge the DNF representations of multiple rules.  Rules are combined as a conjuntion.
    ///
    /// eg. { A, B } and ({ C } or { D, E }) => { A, B, C } or { A, B, D, E }
    fn from_iter<I: IntoIterator<Item = DNF>>(iter: I) -> DNF {
        DNF(iter
            .into_iter()
            .multi_cartesian_product()
            .map(|x| x.into_iter().flatten().collect())
            .collect())
    }
}

impl DNF {
    // TODO: Remove sets which are a superset of another in the list
    fn minimize(&mut self) {
        self.0.sort_unstable_by_key(|x| x.len());
        self.0.dedup();
        let len = match self.0.first() {
            Some(x) => x.len(),
            None => 0,
        };
        self.0.retain(|x| x.len() == len);
    }

    /// The possible additions to `approvals` which would result in the requirements being met.
    fn fixes(&self, approvals: HashSet<String>) -> DNF {
        let mut ret = Vec::new();
        for reqs in &self.0 {
            ret.push(
                reqs.difference(&approvals)
                    .cloned()
                    .collect::<HashSet<String>>(),
            );
        }
        DNF(ret)
    }
}
