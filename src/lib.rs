#[macro_use]
extern crate failure;
#[macro_use]
extern crate log;
extern crate glob;
extern crate itertools;

use itertools::Itertools;
use std::collections::HashSet;
use std::fmt;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::str::FromStr;

pub type Result<T> = ::std::result::Result<T, failure::Error>;

pub type Name = String;

pub struct RuleSet(pub Vec<Rule>);

/// A rule is satisfied when any `n` members of `pop` approve.
#[derive(Debug, Clone)]
pub struct Rule {
    pub pat: glob::Pattern,
    pub pop: HashSet<String>,
    pub lvl: Level,
    pub n: usize,
}

/// Indicates the level of scrutiny required by a reviewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct Level(pub usize);

// impl Level {
//     pub const MAX: Level = Level(2);
// }

impl RuleSet {
    pub fn from_reader(rdr: impl Read) -> Result<RuleSet> {
        let mut rules = Vec::new();
        for l in BufReader::new(rdr).lines() {
            let mut l = l?;
            if let Some(i) = l.find('#') {
                l.truncate(i);
            }
            if l.is_empty() {
                continue;
            }
            match l.parse::<Rule>() {
                Ok(rule) => rules.push(rule),
                Err(e) => error!("Couldn't parse rule {}: {}", l, e),
            }
        }
        Ok(RuleSet(rules))
    }

    // pub fn cnf_for(&self, path: &Path) -> CNF {
    //     CNF::from_iter(
    //         self.0
    //             .iter()
    //             .filter(|rule| rule.pat.matches(&path.to_string_lossy()))
    //             .map(|rule| CNF::from(rule)),
    //     )
    // }

    pub fn matching(&self, path: &Path) -> RuleSet {
        RuleSet(
            self.0
                .iter()
                .filter(|rule| rule.pat.matches(&path.to_string_lossy()))
                .cloned()
                .collect(),
        )
    }

    pub fn approve(&mut self, name: &str, lvl: Level) {
        for req in &mut self.0 {
            if req.pop.contains(name) && req.lvl <= lvl {
                req.pop.remove(name);
                req.n -= 1;
            }
        }
        self.0.retain(|req| req.n > 0);
    }
}

// Parsing and printing

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for _ in 0..(self.0 + 1) {
            f.write_str("!")?;
        }
        Ok(())
    }
}

impl FromStr for Level {
    type Err = failure::Error;

    fn from_str(line: &str) -> Result<Level> {
        if line.chars().all(|c| c == '!') {
            assert!(line.len() > 0);
            Ok(Level(line.len() - 1))
        } else {
            bail!("Level should be all exclamation marks")
        }
    }
}

// impl fmt::Display for Rule {
//     fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
//         let mut iter = self.0.iter();
//         write!(f, "(")?;
//         if let Some(x) = iter.next() {
//             write!(f, "{}", x)?;
//         }
//         for x in iter {
//             write!(f, " ∧ {}", x)?;
//         }
//         write!(f, ")")
//     }
// }

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}\t{}\t{}\t{}",
            self.pat,
            self.lvl,
            self.n,
            self.pop
                .iter()
                .map(|x| x.as_str())
                .intersperse(",")
                .collect::<String>()
        )
    }
}

impl FromStr for Rule {
    type Err = failure::Error;

    fn from_str(line: &str) -> Result<Rule> {
        let mut ws = line.split_whitespace();
        let pat = glob::Pattern::new(ws.next().unwrap())?;
        let lvl: Level = ws.next().unwrap().parse()?;
        let n: usize = ws.next().unwrap().parse()?;
        let pop: HashSet<String> = ws
            .next()
            .unwrap()
            .split(',')
            .map(|x| x.to_owned())
            .collect();
        if n > pop.len() {
            warn!("Unsatisfiable rule! {}", line);
        }
        Ok(Rule { pat, n, lvl, pop })
    }
}

impl fmt::Display for RuleSet {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for rule in &self.0 {
            writeln!(f, "{}", rule)?;
        }
        Ok(())
    }
}
