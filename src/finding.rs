use crate::rules::{Rule, Severity};

#[derive(Debug)]
pub struct Finding {
    pub path: String,
    /// 1-based, as editors and GitHub count.
    pub line: usize,
    pub col: usize,
    pub rule: &'static Rule,
    pub message: String,
}

impl Finding {
    pub fn severity(&self) -> Severity {
        self.rule.severity
    }
}
