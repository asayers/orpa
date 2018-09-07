use failure;
use glob;
use itertools::Itertools;
use std::collections::HashSet;
use std::fmt;
use std::iter::FromIterator;
use std::str::FromStr;

/// A rule is satisfied when any `n` members of `pop` approve.
#[derive(Debug)]
pub struct Rule {
    pat: glob::Pattern,
    pop: Vec<String>,
    n: usize,
}

impl Rule {
    pub fn matches(&self, s: &str) -> bool {
        self.pat.matches(s)
    }
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
pub struct DNF(Vec<HashSet<String>>);

impl fmt::Display for DNF {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for set in &self.0 {
            writeln!(
                f,
                "{}",
                String::from_iter(set.iter().map(|x| x.as_str()).intersperse(","))
            )?;
        }
        Ok(())
    }
}

impl IntoIterator for DNF {
    type Item = HashSet<String>;
    type IntoIter = ::std::vec::IntoIter<HashSet<String>>;
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
    pub fn minimize(&mut self) {
        self.0.sort_unstable_by_key(|x| x.len());
        self.0.dedup();
        let len = match self.0.first() {
            Some(x) => x.len(),
            None => 0,
        };
        self.0.retain(|x| x.len() == len);
    }

    /// The possible additions to `approvals` which would result in the requirements being met.
    pub fn fixes(&self, approvals: HashSet<String>) -> DNF {
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

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
