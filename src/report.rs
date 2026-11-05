//! Output formats: text for people, GitHub annotations for pull requests, JSON for tools.

use serde::Serialize;

use crate::finding::Finding;
use crate::rules::Severity;

/// Counts for the summary line.
#[derive(Default)]
pub struct Summary {
    pub files: usize,
    /// Turned off by ignore comments.
    pub ignored: usize,
    /// Already in the baseline.
    pub known: usize,
}

pub fn text(findings: &[Finding], totals: &Summary) -> String {
    let mut out = String::new();
    for f in findings {
        out.push_str(&format!(
            "{}:{}:{}  {} {}  {}\n    fix: {}\n",
            f.path,
            f.line,
            f.col,
            f.rule.code,
            f.severity().as_str(),
            f.message,
            f.rule.fix
        ));
    }
    out.push_str(&summary(findings, totals));
    out.push('\n');
    out
}

pub fn github(findings: &[Finding]) -> String {
    let mut out = String::new();
    for f in findings {
        let level = match f.severity() {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "notice",
        };
        let title = format!("{} {}", f.rule.code, f.rule.name);
        let body = format!("{} Fix: {}", f.message, f.rule.fix);
        out.push_str(&format!(
            "::{level} file={},line={},col={},title={}::{}\n",
            escape_property(&f.path),
            f.line,
            f.col,
            escape_property(&title),
            escape_data(&body)
        ));
    }
    out
}

#[derive(Serialize)]
struct JsonFinding<'a> {
    path: &'a str,
    line: usize,
    col: usize,
    rule: &'a str,
    name: &'a str,
    severity: Severity,
    message: &'a str,
    fix: &'a str,
}

pub fn json(findings: &[Finding]) -> String {
    let rows: Vec<JsonFinding> = findings
        .iter()
        .map(|f| JsonFinding {
            path: &f.path,
            line: f.line,
            col: f.col,
            rule: f.rule.code,
            name: f.rule.name,
            severity: f.severity(),
            message: &f.message,
            fix: f.rule.fix,
        })
        .collect();
    serde_json::to_string_pretty(&rows).expect("findings serialize") + "\n"
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn summary(findings: &[Finding], totals: &Summary) -> String {
    let count = |s| findings.iter().filter(|f| f.severity() == s).count();
    let (errors, warnings, notes) = (
        count(Severity::Error),
        count(Severity::Warning),
        count(Severity::Note),
    );
    let checked = plural(totals.files, "file");
    let mut line = if errors + warnings == 0 {
        format!("All clear: checked {checked}, no calls that can wait forever.")
    } else {
        format!(
            "Found {} and {} in {checked}.",
            plural(errors, "error"),
            plural(warnings, "warning")
        )
    };
    if notes > 0 {
        line.push_str(&format!(" {} couldn't be checked.", plural(notes, "call")));
    }
    if totals.ignored > 0 {
        line.push_str(&format!(
            " {} ignored by comments.",
            plural(totals.ignored, "finding")
        ));
    }
    if totals.known > 0 {
        line.push_str(&format!(
            " {} already in the baseline.",
            plural(totals.known, "finding")
        ));
    }
    line
}

fn escape_data(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

fn escape_property(value: &str) -> String {
    escape_data(value).replace(':', "%3A").replace(',', "%2C")
}
