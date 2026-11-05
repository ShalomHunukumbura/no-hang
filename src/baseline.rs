//! A baseline: the findings a codebase already has, saved so that only new ones fail.
//!
//! Entries are matched by file, rule and message, with a count, not by line number, so
//! editing other parts of a file doesn't bring old findings back. The message names the
//! call (`requests.post() has no timeout...`), which tells calls in one file apart.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::finding::Finding;

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    findings: Vec<Entry>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    path: String,
    rule: String,
    message: String,
    count: usize,
}

type Key = (String, String, String);

fn key(f: &Finding) -> Key {
    (f.path.clone(), f.rule.code.to_string(), f.message.clone())
}

/// The baseline file's contents for these findings, sorted so it diffs cleanly.
pub fn render(findings: &[Finding]) -> String {
    let mut counts: BTreeMap<Key, usize> = BTreeMap::new();
    for f in findings {
        *counts.entry(key(f)).or_default() += 1;
    }
    let file = File {
        version: 1,
        findings: counts
            .into_iter()
            .map(|((path, rule, message), count)| Entry {
                path,
                rule,
                message,
                count,
            })
            .collect(),
    };
    serde_json::to_string_pretty(&file).expect("a baseline serializes") + "\n"
}

pub struct Baseline {
    counts: BTreeMap<Key, usize>,
}

impl Baseline {
    pub fn parse(json: &str) -> Result<Baseline, String> {
        let file: File = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if file.version != 1 {
            return Err(format!("version {} isn't supported", file.version));
        }
        let mut counts = BTreeMap::new();
        for e in file.findings {
            *counts.entry((e.path, e.rule, e.message)).or_default() += e.count;
        }
        Ok(Baseline { counts })
    }

    /// Drop the findings the baseline already has. Returns the rest, how many were dropped,
    /// and how many baseline entries no longer happen (fixed, so they can be removed).
    pub fn filter(mut self, findings: Vec<Finding>) -> (Vec<Finding>, usize, usize) {
        let mut kept = Vec::new();
        let mut known = 0;
        for f in findings {
            match self.counts.get_mut(&key(&f)) {
                Some(n) if *n > 0 => {
                    *n -= 1;
                    known += 1;
                }
                _ => kept.push(f),
            }
        }
        let fixed = self.counts.values().sum();
        (kept, known, fixed)
    }
}
