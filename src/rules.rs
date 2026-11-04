//! The rule catalog. Codes are stable: once released, a code keeps its meaning.

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    /// Reported, never fails the run.
    Note,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Note => "note",
        }
    }
}

#[derive(Debug)]
pub struct Rule {
    pub code: &'static str,
    pub name: &'static str,
    pub severity: Severity,
    pub summary: &'static str,
    pub why: &'static str,
    pub fix: &'static str,
}

pub const HTTP_NO_TIMEOUT: &str = "NH001";
pub const TIMEOUT_NONE: &str = "NH002";
pub const SUBPROCESS_NO_TIMEOUT: &str = "NH003";
pub const CANT_CHECK: &str = "NH900";

pub const RULES: &[Rule] = &[
    Rule {
        code: HTTP_NO_TIMEOUT,
        name: "http-no-timeout",
        severity: Severity::Error,
        summary: "An HTTP request is made with no timeout, so it waits forever if the server \
                  stops answering.",
        why: "requests has no default timeout. If the server accepts the connection and then \
              never answers, the call blocks for as long as the process lives. In a web app \
              each stuck call holds a worker; once every worker is waiting, the whole site \
              stops responding, even for pages that never touch the slow server.",
        fix: "Pass timeout=(connect, read) in seconds, e.g. timeout=(3.05, 30). The read \
              timeout is the longest gap between bytes, not the total time.",
    },
    Rule {
        code: TIMEOUT_NONE,
        name: "timeout-none",
        severity: Severity::Error,
        summary: "timeout=None is passed, which turns the timeout off.",
        why: "timeout=None means \"wait forever\", the same as leaving it out. It often comes \
              from a settings value that was never set, or from copying a signature's default.",
        fix: "Pass a number of seconds. If the value comes from settings, give the setting a \
              default instead of None.",
    },
    Rule {
        code: SUBPROCESS_NO_TIMEOUT,
        name: "subprocess-no-timeout",
        severity: Severity::Warning,
        summary: "A subprocess is run with no timeout, so a stuck command blocks the caller \
                  forever.",
        why: "subprocess.run, call, check_call and check_output wait for the command to exit. \
              A command that waits on the network, a lock or input never exits, and neither \
              does the caller. It's a warning, not an error, because some commands, like a \
              build, are expected to run as long as they need.",
        fix: "Pass timeout= in seconds. The call raises subprocess.TimeoutExpired and kills the \
              command when it runs out.",
    },
    Rule {
        code: CANT_CHECK,
        name: "cant-check",
        severity: Severity::Note,
        summary: "The call passes **kwargs, so a timeout may be inside them. It can't be checked \
                  from the code alone.",
        why: "no-hang reads code without running it. When arguments come from a dictionary \
              built somewhere else, it can't tell whether a timeout is set. It says so rather \
              than guessing either way.",
        fix: "Pass timeout= explicitly next to **kwargs, or make sure the dictionary always \
              has one.",
    },
];

pub fn find(code: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.code.eq_ignore_ascii_case(code))
}
