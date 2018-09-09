use rules::*;
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub struct Requirements(Vec<(Scrutiny, usize, HashSet<String>)>);

impl Requirements {
    pub fn new() -> Requirements {
        Requirements(Vec::new())
    }
    pub fn add(&mut self, lvl: Scrutiny, n: usize, pop: HashSet<String>) {
        self.0.push((lvl, n, pop));
    }
    pub fn approve(&mut self, name: String, lvl: Scrutiny) {
        for req in &mut self.0 {
            if req.2.contains(&name) && req.0 <= lvl {
                req.2.remove(&name);
                req.1 -= 1;
            }
        }
    }
    pub fn normalize(&mut self) {
        self.0.retain(|req| req.1 > 0);
    }
    pub fn is_satisfied(&self) -> bool {
        self.0.is_empty()
    }
}
