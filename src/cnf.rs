use failure;
use itertools::Itertools;
use rules::*;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::hash::Hash;
use std::io::{BufRead, BufReader, Read};
use std::iter::FromIterator;
use std::path::Path;
use std::str::FromStr;

pub type CNF<'a> = Conjunction<Disjunction<'a>>;

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Conjunction<T>(BTreeSet<T>);

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Disjunction<'a>(BTreeMap<&'a str, usize>);

impl<'a> CNF<'a> {
    pub fn discharge<'b>(&mut self, name: &'b str, lvl: usize) {
        let mut old = Conjunction(BTreeSet::new());
        ::std::mem::swap(self, &mut old);
        for disjunction in old.0 {
            match disjunction.0.get(name) {
                Some(&x) if lvl >= x => { /*discharged*/ }
                _ => {
                    self.0.insert(disjunction);
                }
            }
        }
    }
}

// impl<'a> Disjunction<Atom<'a>> {
// fn insert(&mut self, x: Atom<'a>) {
//     let old = BTreeSet::new();
//     ::std::mem::swap(self.0, &mut old);
//     for o in old {
//         if o.name == x.name && o.lvl >= x.name {

//         }
//     }
// }
// }

impl<'a> From<&'a Rule> for CNF<'a> {
    fn from(rule: &'a Rule) -> CNF<'a> {
        let disjunction_len = rule.pop.len() + 1 - rule.n;
        let mut conjunction = Conjunction(BTreeSet::new());
        for names in rule.pop.iter().combinations(disjunction_len) {
            let mut disjunction = Disjunction(BTreeMap::new());
            for name in names {
                disjunction.0.insert(name, rule.lvl);
            }
            conjunction.0.insert(disjunction);
        }
        conjunction
    }
}

impl<'a> FromIterator<CNF<'a>> for CNF<'a> {
    fn from_iter<I: IntoIterator<Item = CNF<'a>>>(iter: I) -> CNF<'a> {
        let mut ret = BTreeSet::new();
        for i in iter {
            ret.extend(i.0);
        }
        Conjunction(ret)
    }
}

impl<T: fmt::Display> fmt::Display for Conjunction<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut iter = self.0.iter();
        write!(f, "(")?;
        if let Some(x) = iter.next() {
            write!(f, "{}", x)?;
        }
        for x in iter {
            write!(f, " ∧ {}", x)?;
        }
        write!(f, ")")
    }
}

impl<'a> fmt::Display for Disjunction<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut iter = self.0.iter();
        write!(f, "(")?;
        if let Some((k, v)) = iter.next() {
            write!(f, "{}{}", k, v)?;
        }
        for (k, v) in iter {
            write!(f, " ∨ {}{}", k, v)?;
        }
        write!(f, ")")
    }
}

#[test]
fn foo() {
    let rule = Rule {
        pat: ::glob::Pattern::new("*").unwrap(),
        pop: [
            "A".to_string(),
            "B".to_string(),
            "C".to_string(),
            "D".to_string(),
        ].iter()
            .cloned()
            .collect(),
        lvl: 1,
        n: 3,
    };
    println!(
        "{}",
        CNF::from(&rule) // .discharge(Atom(&"D".to_string(), 1))
                         // .discharge(Atom(&"C".to_string(), 1))
    );
}
