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
    fix: Pass timeout= explicitly next to **kwargs, or make sure the dictionary always has one.
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

That covers `with requests.Session() as s`, a parameter typed `requests.Session`, and
`requests.Session().get(url)`. A session that mounts an adapter or wraps `request` may set
a timeout no-hang can't see, so it's left alone. A name means one thing per function:
`s` in a function that takes `s` as a parameter isn't the module's session.

It knows which libraries are already bounded. `httpx` times out after 5 seconds and
`aiohttp` after 5 minutes, so their plain calls aren't reported, but turning the default off
is: `httpx.Client(timeout=None)`, `httpx.Timeout(None)`, or an `aiohttp.ClientTimeout()`
with no `total` or `sock_read` (every field defaults to None).

On 14,000 files of installed packages it takes under 4 seconds, and found real ones in
fsspec, tqdm's Telegram and Discord notifiers, transformers and redis-py's OCSP check.

**Status: in development, not released yet.** Python `requests`, `httpx`, `aiohttp` and
`subprocess` work today. Coming next: databases, Redis, `urllib`, `smtplib`, sockets and
boto3, then JavaScript, TypeScript and Go, and prebuilt binaries with a GitHub Action.

## Rules

| Code | Name | Severity | Finds |
|---|---|---|---|
| NH001 | http-no-timeout | error | A `requests` call, or a call on a `requests.Session`, with no `timeout=` |
| NH002 | timeout-none | error | `timeout=None`, `httpx.Timeout(None)` or an `aiohttp.ClientTimeout()` with no read limit |
| NH003 | subprocess-no-timeout | warning | `subprocess.run`, `call`, `check_call` or `check_output` with no `timeout=` |
| NH900 | cant-check | note | A call that passes `**kwargs`, where a timeout may be hidden |

`no-hang --explain NH001` says why a rule exists and how to fix what it finds.

## Usage

```bash
no-hang                       # check the current directory, skipping .gitignored files
no-hang app/ scripts/run.py   # or point it at files and directories
no-hang --strict              # fail on warnings too
no-hang --ignore NH003        # skip rules
no-hang --format github       # annotations on the pull request diff
no-hang --format json         # for other tools
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
