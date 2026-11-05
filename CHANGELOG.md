# Changelog

## 0.1.0

The first release.

- Python: `requests` (calls and sessions, followed from where they're made), `httpx` and
  `aiohttp` with their timeouts turned off, `urllib`, `http.client`, `smtplib`, `ftplib`,
  `poplib`, `imaplib`, sockets, `subprocess`, redis-py, asyncpg, psycopg and SQLAlchemy.
- JavaScript and TypeScript: axios, `child_process`, node-postgres and ioredis.
- Go: `net/http`, `os/exec` and `database/sql`.
- Rules NH001 to NH006 and NH900, each with `--explain` and a fix for each language.
- Ignore comments (`# no-hang: ignore[NH003] reason`), a baseline file, `--no-ignore`.
- Text, GitHub annotation and JSON output.
- Prebuilt binaries for Linux, macOS and Windows, and a GitHub Action.
