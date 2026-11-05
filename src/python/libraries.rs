//! What each library does when no timeout is passed. This table is the knowledge the tool
//! holds, and each entry was checked against the library's source:
//!
//! - `requests` has no default timeout, for functions or sessions.
//! - `httpx` times out after 5 seconds and `aiohttp` after 5 minutes, so only turning
//!   those off is a finding.
//! - The standard library's network clients (`urlopen`, `http.client`, `smtplib`, `ftplib`,
//!   `poplib`, `imaplib`, `socket.create_connection`) use the socket module's global
//!   default, which is None (wait forever) unless `socket.setdefaulttimeout` was called.
//! - redis-py 6: `socket_timeout=None`, and the connect timeout falls back to it. A URL can
//!   carry `?socket_timeout=`.
//! - asyncpg: connecting is bounded (`timeout=60`), queries aren't (`command_timeout=None`).
//! - psycopg 2 and 3 hand connect_timeout to libpq, where "not specified means wait
//!   indefinitely". It can come in the connection string.
//! - boto3 has 60 second connect and read timeouts, so it isn't listed.

use crate::rules::{
    DB_NO_TIMEOUT, HTTP_NO_TIMEOUT, REDIS_NO_TIMEOUT, SOCKET_NO_TIMEOUT, SUBPROCESS_NO_TIMEOUT,
};

/// How a call's timeout is passed.
pub struct Spec {
    /// The keyword argument that bounds the call.
    pub param: &'static str,
    /// Its position when it's passed without the keyword, counting from 0.
    pub position: Option<usize>,
    /// Keywords that hand the timeout to something else, like `connection_pool=`.
    pub delegated: &'static [&'static str],
    /// The keyword a URL or connection string comes in (it can also be the first argument),
    /// which may carry `param=` itself.
    pub url: Option<&'static str>,
    /// Bounded by `socket.setdefaulttimeout` when it isn't passed.
    pub global_default: bool,
}

const fn timeout_at(position: Option<usize>) -> Spec {
    Spec {
        param: "timeout",
        position,
        delegated: &[],
        url: None,
        global_default: false,
    }
}

const fn socket_default(position: Option<usize>) -> Spec {
    Spec {
        global_default: true,
        ..timeout_at(position)
    }
}

/// A function or class that blocks.
pub struct Function {
    pub names: &'static [&'static str],
    pub spec: Spec,
    /// The rule for leaving the timeout out, or None when the library has a default
    /// (then only an explicit `timeout=None` is a finding).
    pub missing: Option<&'static str>,
    /// What goes wrong, to finish "has no timeout, so ...".
    pub consequence: &'static str,
}

const SERVER_HANGS: &str = "it waits forever if the server stops answering";

pub const FUNCTIONS: &[Function] = &[
    Function {
        names: &[
            "requests.get",
            "requests.post",
            "requests.put",
            "requests.patch",
            "requests.delete",
            "requests.head",
            "requests.options",
            "requests.request",
        ],
        spec: timeout_at(None),
        missing: Some(HTTP_NO_TIMEOUT),
        consequence: SERVER_HANGS,
    },
    Function {
        names: &[
            "subprocess.run",
            "subprocess.call",
            "subprocess.check_call",
            "subprocess.check_output",
        ],
        spec: timeout_at(None),
        missing: Some(SUBPROCESS_NO_TIMEOUT),
        consequence: "a command that never exits blocks forever",
    },
    Function {
        names: &[
            "httpx.get",
            "httpx.post",
            "httpx.put",
            "httpx.patch",
            "httpx.delete",
            "httpx.head",
            "httpx.options",
            "httpx.request",
            "httpx.stream",
            "aiohttp.request",
        ],
        spec: timeout_at(None),
        missing: None,
        consequence: SERVER_HANGS,
    },
    // urlopen(url, data=None, timeout=...)
    Function {
        names: &["urllib.request.urlopen"],
        spec: socket_default(Some(2)),
        missing: Some(HTTP_NO_TIMEOUT),
        consequence: SERVER_HANGS,
    },
    // HTTPConnection(host, port=None, timeout=...); HTTPSConnection takes it by keyword.
    Function {
        names: &["http.client.HTTPConnection"],
        spec: socket_default(Some(2)),
        missing: Some(HTTP_NO_TIMEOUT),
        consequence: SERVER_HANGS,
    },
    Function {
        names: &["http.client.HTTPSConnection"],
        spec: socket_default(None),
        missing: Some(HTTP_NO_TIMEOUT),
        consequence: SERVER_HANGS,
    },
    // create_connection(address, timeout=...)
    Function {
        names: &["socket.create_connection"],
        spec: socket_default(Some(1)),
        missing: Some(SOCKET_NO_TIMEOUT),
        consequence: SERVER_HANGS,
    },
    // SMTP(host, port, local_hostname, timeout)
    Function {
        names: &["smtplib.SMTP"],
        spec: socket_default(Some(3)),
        missing: Some(SOCKET_NO_TIMEOUT),
        consequence: SERVER_HANGS,
    },
    // FTP(host, user, passwd, acct, timeout)
    Function {
        names: &["ftplib.FTP"],
        spec: socket_default(Some(4)),
        missing: Some(SOCKET_NO_TIMEOUT),
        consequence: SERVER_HANGS,
    },
    // POP3(host, port, timeout) and IMAP4(host, port, timeout)
    Function {
        names: &["poplib.POP3", "imaplib.IMAP4"],
        spec: socket_default(Some(2)),
        missing: Some(SOCKET_NO_TIMEOUT),
        consequence: SERVER_HANGS,
    },
    // The TLS versions take the timeout by keyword only.
    Function {
        names: &[
            "smtplib.SMTP_SSL",
            "ftplib.FTP_TLS",
            "poplib.POP3_SSL",
            "imaplib.IMAP4_SSL",
        ],
        spec: socket_default(None),
        missing: Some(SOCKET_NO_TIMEOUT),
        consequence: SERVER_HANGS,
    },
    Function {
        names: &[
            "redis.Redis",
            "redis.StrictRedis",
            "redis.ConnectionPool",
            "redis.client.Redis",
            "redis.asyncio.Redis",
            "redis.asyncio.StrictRedis",
            "redis.asyncio.ConnectionPool",
            "redis.cluster.RedisCluster",
            "redis.RedisCluster",
            "redis.asyncio.RedisCluster",
        ],
        spec: Spec {
            param: "socket_timeout",
            position: None,
            delegated: &["connection_pool"],
            url: None,
            global_default: false,
        },
        missing: Some(REDIS_NO_TIMEOUT),
        consequence: "a command waits forever if Redis stops answering",
    },
    Function {
        names: &[
            "redis.from_url",
            "redis.Redis.from_url",
            "redis.StrictRedis.from_url",
            "redis.ConnectionPool.from_url",
            "redis.asyncio.from_url",
            "redis.asyncio.Redis.from_url",
            "redis.asyncio.ConnectionPool.from_url",
        ],
        spec: Spec {
            param: "socket_timeout",
            position: None,
            delegated: &["connection_pool"],
            url: Some("url"),
            global_default: false,
        },
        missing: Some(REDIS_NO_TIMEOUT),
        consequence: "a command waits forever if Redis stops answering",
    },
    Function {
        names: &["asyncpg.connect", "asyncpg.create_pool"],
        spec: Spec {
            param: "command_timeout",
            position: None,
            delegated: &[],
            url: None,
            global_default: false,
        },
        missing: Some(DB_NO_TIMEOUT),
        consequence: "a query can run forever",
    },
    Function {
        names: &[
            "psycopg2.connect",
            "psycopg.connect",
            "psycopg.Connection.connect",
            "psycopg.AsyncConnection.connect",
        ],
        spec: Spec {
            param: "connect_timeout",
            position: None,
            delegated: &[],
            url: Some("dsn"),
            global_default: false,
        },
        missing: Some(DB_NO_TIMEOUT),
        consequence: "connecting waits indefinitely if the database host doesn't answer",
    },
];

pub fn function_for(qualified: &str) -> Option<&'static Function> {
    FUNCTIONS.iter().find(|f| f.names.contains(&qualified))
}

/// Every finding needs one of these modules imported, so a file that never mentions any
/// of them is skipped without parsing it. Most files in a big project are.
pub const MODULES: &[&str] = &[
    "requests",
    "subprocess",
    "httpx",
    "aiohttp",
    "urllib",
    "http.client",
    "from http",
    "socket",
    "smtplib",
    "ftplib",
    "poplib",
    "imaplib",
    "redis",
    "asyncpg",
    "psycopg",
];

/// Objects that are made in one place and make requests in another.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Library {
    Requests,
    Httpx,
    Aiohttp,
    /// `urllib.request.build_opener()`, whose `open` works like urlopen.
    Opener,
    /// `socket.socket()`, which blocks on connect unless settimeout is called.
    Socket,
}

const VERBS: &[&str] = &[
    "get", "post", "put", "patch", "delete", "head", "options", "request",
];

impl Library {
    pub fn methods(self) -> &'static [&'static str] {
        match self {
            Library::Requests => VERBS,
            Library::Httpx => &[
                "get", "post", "put", "patch", "delete", "head", "options", "request", "stream",
            ],
            Library::Aiohttp => &[
                "get",
                "post",
                "put",
                "patch",
                "delete",
                "head",
                "options",
                "request",
                "ws_connect",
            ],
            Library::Opener => &["open"],
            Library::Socket => &["connect", "connect_ex"],
        }
    }

    /// How a method's timeout is passed.
    pub fn spec(self) -> Spec {
        match self {
            // open(fullurl, data=None, timeout=...)
            Library::Opener => socket_default(Some(2)),
            Library::Socket => socket_default(None),
            _ => timeout_at(None),
        }
    }
}

pub const CLIENTS: &[(&str, Library)] = &[
    ("requests.Session", Library::Requests),
    ("requests.session", Library::Requests),
    ("requests.sessions.Session", Library::Requests),
    ("httpx.Client", Library::Httpx),
    ("httpx.AsyncClient", Library::Httpx),
    ("aiohttp.ClientSession", Library::Aiohttp),
    ("urllib.request.build_opener", Library::Opener),
    ("socket.socket", Library::Socket),
];

pub fn client_class(qualified: &str) -> Option<Library> {
    CLIENTS
        .iter()
        .find(|(name, _)| *name == qualified)
        .map(|(_, lib)| *lib)
}

/// Calling these on a client sets a timeout we can't see: a mounted adapter, or a socket's
/// own timeout.
pub fn customising(method: &str) -> bool {
    matches!(method, "mount" | "settimeout" | "setblocking")
}

/// Assigning these on a client replaces how it sends requests.
pub fn replaces_sending(attr: &str) -> bool {
    VERBS.contains(&attr) || attr == "send"
}
