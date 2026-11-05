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
pub const SOCKET_NO_TIMEOUT: &str = "NH004";
pub const REDIS_NO_TIMEOUT: &str = "NH005";
pub const DB_NO_TIMEOUT: &str = "NH006";
pub const CANT_CHECK: &str = "NH900";

pub const RULES: &[Rule] = &[
    Rule {
        code: HTTP_NO_TIMEOUT,
        name: "http-no-timeout",
        severity: Severity::Error,
        summary: "An HTTP request is made with no timeout, so it waits forever if the server \
                  stops answering.",
        why: "requests has no default timeout, and neither does a requests.Session (setting \
              session.timeout does nothing), and urllib's urlopen and http.client wait \
              forever too unless socket.setdefaulttimeout() was called. axios's default \
              timeout is 0, meaning none. Go's http.Get, http.Post and http.DefaultClient have \
              no timeout, and neither does an http.Client{} without one. If the server accepts \
              the connection and then never answers, the call blocks for as long as the process lives. In a web app \
              each stuck call holds a worker; once every worker is waiting, the whole site \
              stops responding, even for pages that never touch the slow server.",
        fix: "Python: pass timeout=(connect, read) in seconds, e.g. timeout=(3.05, 30); the \
              read timeout is the longest gap between bytes, not the total time. axios: pass \
              { timeout: 10000 } in milliseconds, or set it once with axios.create or \
              axios.defaults.timeout. Go: make an http.Client{Timeout: 10 * time.Second}, or \
              build requests with http.NewRequestWithContext and a context.WithTimeout.",
    },
    Rule {
        code: TIMEOUT_NONE,
        name: "timeout-none",
        severity: Severity::Error,
        summary: "A timeout is set to None, which turns it off.",
        why: "timeout=None means \"wait forever\", the same as leaving it out. It often comes \
              from a settings value that was never set, or from copying a signature's default. \
              It hides in client and timeout objects too: httpx.Client(timeout=None) turns off \
              the 5 second default for every request the client makes, httpx.Timeout(None) \
              sets every limit to None, and aiohttp.ClientTimeout() with no total or sock_read \
              leaves reads unbounded, because every field defaults to None. In JavaScript, \
              { timeout: 0 } means no timeout for axios and child_process, and Go's \
              http.Client{Timeout: 0} is the same as leaving it out.",
        fix: "Pass a number of seconds. If the value comes from settings, give the setting a \
              default instead of None. For aiohttp.ClientTimeout, set total or sock_read. In \
              JavaScript and Go, pass a number above zero.",
    },
    Rule {
        code: SUBPROCESS_NO_TIMEOUT,
        name: "subprocess-no-timeout",
        severity: Severity::Warning,
        summary: "A subprocess is run with no timeout, so a stuck command blocks the caller \
                  forever.",
        why: "Python's subprocess.run, call, check_call and check_output, Node's execSync, \
              exec, execFile, execFileSync and spawnSync, and Go's exec.Command wait for the \
              command to exit. \
              A command that waits on the network, a lock or input never exits, and neither \
              does the caller. It's a warning, not an error, because some commands, like a \
              build, are expected to run as long as they need.",
        fix: "Python: pass timeout= in seconds; the call raises subprocess.TimeoutExpired and \
              kills the command when it runs out. Node: pass { timeout: 60000 } in \
              milliseconds. Go: use exec.CommandContext with a context.WithTimeout.",
    },
    Rule {
        code: SOCKET_NO_TIMEOUT,
        name: "socket-no-timeout",
        severity: Severity::Error,
        summary: "A socket, mail or FTP connection is opened with no timeout, so it waits \
                  forever if the server stops answering.",
        why: "socket.create_connection, smtplib, ftplib, poplib, imaplib and a socket.socket \
              that connects without settimeout() all use the socket module's global default, \
              which is None: wait forever. A mail server that accepts the connection and then \
              stalls holds the request that's sending the email, often a sign-up or a \
              password reset. Calling socket.setdefaulttimeout() in the same file bounds them, \
              and no-hang takes that into account.",
        fix: "Pass timeout= in seconds, e.g. smtplib.SMTP(host, 587, timeout=10), or call \
              sock.settimeout(10) before connecting.",
    },
    Rule {
        code: REDIS_NO_TIMEOUT,
        name: "redis-no-timeout",
        severity: Severity::Error,
        summary: "A Redis client is made with no command timeout, so a command waits forever if \
                  Redis stops answering.",
        why: "redis-py's socket_timeout defaults to None, and the connect timeout falls back to \
              it. ioredis bounds connecting (10 seconds) but has no commandTimeout by \
              default. A cache is meant to make pages faster; with no timeout, a Redis that's \
              stalled (a failover, a full disk, a long KEYS or SAVE) makes every page that \
              reads the cache hang instead of falling back.",
        fix: "redis-py: pass socket_timeout= and socket_connect_timeout= in seconds, or add \
              ?socket_timeout=5 to the URL. ioredis: pass { commandTimeout: 2000 }. For blocking commands like BLPOP, make \
              socket_timeout longer than the command's own timeout, or use a separate client.",
    },
    Rule {
        code: DB_NO_TIMEOUT,
        name: "db-no-timeout",
        severity: Severity::Warning,
        summary: "A database connection is opened with no timeout, so a query or a connection \
                  attempt can wait forever.",
        why: "asyncpg bounds connecting but not queries: command_timeout defaults to None, so a \
              query stuck behind a lock waits as long as the lock is held. psycopg leaves \
              connect_timeout to libpq, which waits indefinitely when it isn't set. \
              node-postgres has no connectionTimeoutMillis by default, so when a Pool's \
              connections are all busy the next query waits for one forever. Go's \
              database/sql Query and Exec have no deadline. It's a \
              warning, not an error, because the database can bound queries itself with a \
              statement_timeout set on the server or the role, which the code doesn't show.",
        fix: "asyncpg: pass command_timeout= in seconds. psycopg: pass connect_timeout= or put \
              connect_timeout=10 in the connection string, and set statement_timeout on the \
              server or in options='-c statement_timeout=30s'. node-postgres: pass \
              connectionTimeoutMillis: 5000. Go: use QueryContext, ExecContext and the rest \
              with a context.WithTimeout.",
    },
    Rule {
        code: CANT_CHECK,
        name: "cant-check",
        severity: Severity::Note,
        summary: "The call's timeout may come from **kwargs or a connection string that's \
                  built elsewhere, so it can't be checked from the code alone.",
        why: "no-hang reads code without running it. When arguments come from a dictionary, \
              or a Redis or Postgres URL comes from settings, it can't tell whether a timeout \
              is set. It says so rather than guessing either way.",
        fix: "Pass the timeout explicitly next to **kwargs or the URL (socket_timeout=, \
              connect_timeout=), or make sure the dictionary or URL always has one.",
    },
];

pub fn find(code: &str) -> Option<&'static Rule> {
    RULES.iter().find(|r| r.code.eq_ignore_ascii_case(code))
}
