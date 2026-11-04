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
    fix: Pass a number of seconds. If the value comes from settings, give the setting a default instead of None.
app/payments.py:28:35  NH900 note  requests.post(**options) may set timeout inside the dictionary, so it can't be checked.
    fix: Pass timeout= explicitly next to **kwargs, or make sure the dictionary always has one.
app/payments.py:32:5  NH003 warning  subprocess.run() has no timeout, so a command that never exits blocks forever.
    fix: Pass timeout= in seconds. The call raises subprocess.TimeoutExpired and kills the command when it runs out.
Found 2 errors and 1 warning in 2 files. 1 call couldn't be checked.
```

It parses each file with [tree-sitter](https://tree-sitter.github.io/), so a call in a
comment or a string is never reported, and `import requests as r` or
`from requests import post` are followed to the real function. It knows which libraries
are already bounded: `httpx` times out after 5 seconds by default, so `httpx.get(url)` is
fine and isn't reported.

**Status: in development, not released yet.** Python `requests` and `subprocess` work today.
Coming next: clients made in one place and used in another (`requests.Session()`,
`httpx.Client(timeout=None)`), databases, Redis, `urllib` and `smtplib`, then JavaScript,
TypeScript and Go, and prebuilt binaries with a GitHub Action.

## Rules

| Code | Name | Severity | Finds |
|---|---|---|---|
| NH001 | http-no-timeout | error | A `requests` call with no `timeout=` |
| NH002 | timeout-none | error | `timeout=None`, which turns the timeout off |
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
