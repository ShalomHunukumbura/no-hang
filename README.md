# no-hang

**Find network, database and process calls that can wait forever, before one takes your
site down.**

This line passes code review and every test:

```python
requests.post(GATEWAY_URL, json=payment)
```

`requests` has no default timeout. The day the payment gateway accepts connections but stops
answering, every worker that calls it waits forever. The pool fills up, and the whole site
stops responding, including pages that never touch the gateway. Tests don't catch it,
because tests mock the network or the network answers fast.

no-hang reads your code and finds those calls:

```console
$ no-hang
app/payments.py:12:12  NH001 error  requests.post() has no timeout, so it waits forever if the server stops answering.
    fix: Pass timeout=(connect, read) in seconds, e.g. timeout=(3.05, 30). The read timeout is the longest gap between bytes, not the total time.
app/payments.py:24:55  NH002 error  requests.get(timeout=None) turns the timeout off.
    fix: Pass a number of seconds. If the value comes from settings, give the setting a default instead of None. For aiohttp.ClientTimeout, set total or sock_read.
app/payments.py:28:35  NH900 note  requests.post(**options) may set timeout inside the dictionary, so it can't be checked.
    fix: Pass the timeout explicitly next to the **kwargs, options or URL, or make sure they always carry one.
app/payments.py:32:5  NH003 warning  subprocess.run() has no timeout, so a command that never exits blocks forever.
    fix: Pass timeout= in seconds. The call raises subprocess.TimeoutExpired and kills the command when it runs out.
Found 2 errors and 1 warning in 2 files. 1 call couldn't be checked.
```

It parses each file with [tree-sitter](https://tree-sitter.github.io/), so a call in a
comment or a string is never reported, and `import requests as r` or
`from requests import post` are followed to the real function.

It follows clients from where they're made to where they're used:

```python
class Api:
    def __init__(self):
        self.session = requests.Session()
        self.session.timeout = 10       # does nothing: requests ignores it

    def fetch(self, path):
        return self.session.get(BASE + path)   # NH001, and it says why .timeout didn't help
```

That covers `with requests.Session() as s` and `requests.Session().get(url)`. A session
that mounts an adapter or wraps `request` may set a timeout no-hang can't see, so it's left
alone, and so is one passed in as a parameter, which the caller set up (only `timeout=None`
on it is reported). A name means one thing per function: `s` in a function that takes `s`
as a parameter isn't the module's session.

It knows which libraries are already bounded. `httpx` times out after 5 seconds and
`aiohttp` after 5 minutes, so their plain calls aren't reported, but turning the default off
is: `httpx.Client(timeout=None)`, `httpx.Timeout(None)`, or an `aiohttp.ClientTimeout()`
with no `total` or `sock_read` (every field defaults to None).

It covers the standard library, where everything waits forever by default: `urlopen`,
`http.client`, `smtplib`, `ftplib`, `poplib`, `imaplib`, `socket.create_connection` and a
`socket.socket` that connects without `settimeout()`. A file that calls
`socket.setdefaulttimeout(10)` has bounded all of them, and no-hang takes that into account.

And the clients that hang a whole site when the box they talk to stalls: `redis.Redis`
and `from_url` with no `socket_timeout` (unless the URL has `?socket_timeout=`), asyncpg
with no `command_timeout`, and psycopg with no `connect_timeout`. SQLAlchemy's `create_engine` is read the same way:
the URL's scheme says which driver, and `connect_args` or the URL's query has to carry its
timeout (the pool itself gives up after 30 seconds). A connection string that comes from
settings can't be read, so those calls get a note, not an error.

### JavaScript and TypeScript

```ts
const api = axios.create({ baseURL: API });     // NH001: axios's default timeout is 0, none
const pool = new Pool({ connectionString });    // NH006: a full pool queues the caller forever
const redis = new Redis(REDIS_URL);             // NH900: a URL or options it can't read
execSync("git fetch");                          // NH003
```

axios calls and `axios.create()` without `timeout` (unless the file sets
`defaults.timeout`), `child_process` exec and sync calls, node-postgres `Pool` and `Client`
without `connectionTimeoutMillis`, and ioredis without `commandTimeout`. `fetch` isn't
reported: Node's fetch has 300 second header and body timeouts. `.js`, `.mjs`, `.cjs`,
`.jsx`, `.ts`, `.mts`, `.cts` and `.tsx` files are read; `.d.ts` files are skipped.

### Go

`http.Get`, `http.Post` and `http.DefaultClient`, which have no timeout; an
`http.Client{}` without `Timeout`, unless the file builds requests with
`http.NewRequestWithContext` or the client's Transport sets `ResponseHeaderTimeout`;
`exec.Command` (use `exec.CommandContext`); and `database/sql` calls on a handle from
`sql.Open` that don't take a context.

### On real code

On 14,000 files of installed Python packages it takes about 6 seconds, and found real ones
in fsspec, tqdm's Telegram and Discord notifiers, transformers, torch's Redis cache,
setuptools, pre-commit and redis-py's own OCSP check. On Go's own source (7,700 files) it
takes 2 seconds.

boto3 isn't checked: it already has 60 second timeouts.

## Install

Download the binary for your system from the
[latest release](https://github.com/ShalomHunukumbura/no-hang/releases/latest): Linux
(x86_64 and ARM, static, any distribution), macOS (Intel and Apple Silicon) and Windows.
Each archive has a `.sha256` next to it.

```bash
curl -fsSL https://github.com/ShalomHunukumbura/no-hang/releases/latest/download/no-hang-x86_64-unknown-linux-musl.tar.gz | tar xz
./no-hang --version
```

Or build it with Rust: `cargo install --git https://github.com/ShalomHunukumbura/no-hang`.

## GitHub Action

```yaml
name: no-hang
on: [pull_request]
jobs:
  no-hang:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: ShalomHunukumbura/no-hang@v0.1.0
        with:
          args: . --baseline .no-hang-baseline.json   # optional; the default is "."
```

Findings show up on the pull request's diff, the full report goes in the job summary, and
errors fail the check. The action downloads the release matching its own tag and checks
the binary's SHA-256 before running it. It works on Linux, macOS and Windows runners.

## Rules

| Code | Name | Severity | Finds |
|---|---|---|---|
| NH001 | http-no-timeout | error | Python `requests`, `urlopen` and `http.client`; axios; Go's `http.Get` and `http.Client{}`; all with no timeout |
| NH002 | timeout-none | error | `timeout=None`, `httpx.Timeout(None)`, an `aiohttp.ClientTimeout()` with no read limit, `{ timeout: 0 }`, `http.Client{Timeout: 0}` |
| NH003 | subprocess-no-timeout | warning | Python `subprocess`, Node `child_process` exec and sync calls, Go `exec.Command` |
| NH004 | socket-no-timeout | error | `smtplib`, `ftplib`, `poplib`, `imaplib`, `socket.create_connection`, or a socket that connects without `settimeout()` |
| NH005 | redis-no-timeout | error | redis-py with no `socket_timeout`, ioredis with no `commandTimeout` |
| NH006 | db-no-timeout | warning | asyncpg, psycopg, node-postgres and Go `database/sql` with no timeout |
| NH900 | cant-check | note | A call that passes `**kwargs`, or a connection string from settings, where a timeout may be hidden |

`no-hang --explain NH001` says why a rule exists and how to fix what it finds.

## Ignoring a finding

Some calls are meant to wait. Say so where the call starts, with the rule and a reason:

```python
subprocess.run(["make", "release"])  # no-hang: ignore[NH003] a release build takes what it takes

# no-hang: ignore[NH005] this worker blocks on BLPOP by design
queue = redis.Redis(host="queue")
```

`//` works the same in JavaScript, TypeScript and Go. Without codes in brackets, every rule
is ignored on that line. The summary counts what comments turned off, so nothing disappears
silently.

## Adopting it on an existing codebase

```bash
no-hang --write-baseline .no-hang-baseline.json   # save what's there today, and commit it
no-hang --baseline .no-hang-baseline.json         # in CI: fail only on new findings
```

Findings are matched by file, rule and the call they name, not by line number, so editing
the rest of a file doesn't bring old ones back. When one is fixed, no-hang says the baseline
has entries that no longer happen; run `--write-baseline` again to drop them.

## Usage

```bash
no-hang                       # check the current directory, skipping .gitignored files
no-hang app/ scripts/run.py   # or point it at files and directories
no-hang --strict              # fail on warnings too
no-hang --ignore NH003        # skip rules
no-hang --format github       # annotations on the pull request diff
no-hang --format json         # for other tools
no-hang --no-ignore vendor/   # check files .gitignore leaves out
no-hang --baseline FILE       # only new findings fail (see above)
no-hang --list-rules
```

Exit code `0` = clean, `1` = findings that fail the run (errors, or warnings with
`--strict`), `2` = usage error. Notes never fail the run.

## Build from source

```bash
cargo build --release      # the binary is target/release/no-hang
cargo test
```

## License

MIT
