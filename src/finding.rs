use tree_sitter::Node;

use crate::rules::{self, Rule, Severity};

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

impl Finding {
    /// A finding at a node. Codes come from the rule catalog, so a typo fails every test.
    pub fn at(path: &str, node: Node, code: &'static str, message: String) -> Finding {
        let at = node.start_position();
        Finding {
            path: path.to_string(),
            line: at.row + 1,
            col: at.column + 1,
            rule: rules::find(code).expect("every code used is in the catalog"),
            message,
        }
    }
}
