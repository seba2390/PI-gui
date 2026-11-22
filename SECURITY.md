# Security policy

Pistachio starts processes, runs commands over SSH, edits config files that may hold API keys and
fetches web pages for chat web search, so security reports are very welcome.

## Reporting a vulnerability

Please **do not open a public issue.** Report it privately through
[GitHub's security advisories](https://github.com/seba2390/pistachio/security/advisories/new)
with:

- what an attacker can do, and under which conditions,
- steps or a proof of concept to reproduce it,
- the Pistachio version (or commit) and your OS.

You should get an answer within a week. Once a fix is released, the advisory is published with
credit to you (unless you prefer not to be named).

## Supported versions

Only the latest release receives security fixes.

## Scope

In scope: the app and its crates in this repository. That includes how they run pi and ssh,
handle host keys, passwords and credentials, write `models.json` / `auth.json`, and fetch pages
for web search.

Out of scope: vulnerabilities in pi itself (report those to pi), in OpenSSH, or in your own
pi extensions.
