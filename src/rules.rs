use failure;
use glob;
use reqs::*;
use std::fmt;
use std::io::{BufRead, BufReader, Read};
use std::collections::HashSet;
use std::path::Path;
use std::str::FromStr;

type Result<T> = ::std::result::Result<T, failure::Error>;

pub struct RuleSet(Vec<Rule>);

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

    pub fn reqs_for(&self, path: &Path) -> Requirements {
        let mut reqs = Requirements::new();
        for rule in &self.0 {
            if rule.pat.matches(&path.to_string_lossy()) {
                reqs.add(rule.level, rule.n, rule.pop.clone());
            }
        }
        reqs
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct Scrutiny(usize);

impl fmt::Display for Scrutiny {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for _ in 0..self.0 {
            f.write_str("!")?
        }
        Ok(())
    }
}

impl FromStr for Scrutiny {
    type Err = failure::Error;

    fn from_str(line: &str) -> Result<Scrutiny> {
        if line.chars().all(|c| c == '!') {
            Ok(Scrutiny(line.len()))
        } else {
            bail!("Scrutiny field should be made up of !s")
        }
    }
}

/// A rule is satisfied when any `n` members of `pop` approve.
#[derive(Debug, Clone)]
pub struct Rule {
    pub pat: glob::Pattern,
    pub pop: HashSet<String>,
    pub level: Scrutiny,
    pub n: usize,
}

// impl fmt::Display for Rule {
//     fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
//         write!(f, "{}\t{}\t{}\t{}", self.glob, self.level, self.n, self.pop)
//     }
// }

impl FromStr for Rule {
    type Err = failure::Error;

    fn from_str(line: &str) -> Result<Rule> {
        let mut ws = line.split_whitespace();
        let pat = glob::Pattern::new(ws.next().unwrap())?;
        let level: Scrutiny = ws.next().unwrap().parse()?;
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
        Ok(Rule { pat, n, level, pop })
    }
}
