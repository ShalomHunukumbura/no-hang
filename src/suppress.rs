//! Ignore comments: `# no-hang: ignore` or `// no-hang: ignore[NH003] the build takes an hour`,
//! at the end of the finding's line or alone on the line above it. Without codes in brackets
//! every rule is ignored there; anything after the marker is a reason for the reader.

use crate::finding::Finding;

const MARKER: &str = "no-hang:";

/// The rule codes a comment ignores: None if the line has no ignore comment, an empty list
/// for "every rule".
fn ignore_comment(line: &str) -> Option<Vec<String>> {
    let at = line.find(MARKER)?;
    // The marker has to be in a comment, not in a string or code.
    let before = &line[..at];
    if !(before.contains('#') || before.contains("//")) {
        return None;
    }
    let rest = line[at + MARKER.len()..]
        .trim_start()
        .strip_prefix("ignore")?;
    match rest.strip_prefix('[') {
        Some(codes) => {
            let codes = codes.split(']').next()?;
            Some(
                codes
                    .split(',')
                    .map(|c| c.trim().to_ascii_uppercase())
                    .filter(|c| !c.is_empty())
                    .collect(),
            )
        }
        None => Some(Vec::new()),
    }
}

fn covers(line: &str, code: &str) -> bool {
    ignore_comment(line).is_some_and(|codes| codes.is_empty() || codes.iter().any(|c| c == code))
}

fn is_comment_only(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('#') || t.starts_with("//")
}

/// Whether a comment in the source ignores this finding.
pub fn is_ignored(lines: &[&str], finding: &Finding) -> bool {
    let code = finding.rule.code;
    let index = finding.line - 1;
    if lines.get(index).is_some_and(|l| covers(l, code)) {
        return true;
    }
    index
        .checked_sub(1)
        .and_then(|above| lines.get(above))
        .is_some_and(|l| is_comment_only(l) && covers(l, code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ignore_comments() {
        assert_eq!(ignore_comment("x()  # no-hang: ignore"), Some(vec![]));
        assert_eq!(
            ignore_comment("x() // no-hang: ignore[nh003, NH001] builds are slow"),
            Some(vec!["NH003".to_string(), "NH001".to_string()])
        );
        assert_eq!(ignore_comment("s = 'no-hang: ignore'"), None);
        assert_eq!(ignore_comment("# no-hang: please don't"), None);
    }
}
