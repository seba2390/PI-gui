# Sandboxing & Security

What the agent can do on this computer in Code sessions. Sessions on SSH hosts run there,
outside this sandbox. Chats can't reach your files at all: their only tools are web search and
Python in a sealed interpreter.

The settings are saved in `~/.pi/agent/pistachio/sandbox/settings.json` and read by every
command, so changes apply from the next command, also in running sessions.

## Sandbox

The top of the page says which sandbox this computer has: **Seatbelt** on macOS, **bubblewrap**
on Linux, or **Landlock** where bubblewrap is missing or not allowed to run (for example
Ubuntu's AppArmor, or containers). Where bubblewrap is missing, it shows the command that
installs it. Windows has no sandbox yet ([#47](https://github.com/seba2390/pistachio/issues/47)).

Sandbox Code sessions
: On by default. Every `bash` command the agent runs goes through the sandbox, and pi's file
  tools (`read`, `write`, `edit`, and `grep`, `find`, `ls` when turned on) keep to the same
  rules. Not sandboxed: pi itself, its logins and model requests, MCP servers, other extensions,
  and the commands you run yourself with `!`.

In a session, a shield next to the folder in the message box shows the sandbox ("Some sites",
"No network" or "No sandbox" when that applies). Its menu runs that project without a sandbox
(or with one again) and opens this page. When the sandbox blocks a call, the chat says so under
it, with a button that allows reading (or changing) the folder it tried to use.

## Files

Your home folder is hidden from the agent, except the session's folder (for a subfolder of a
repository, the whole repository) and what you allow here. Programs and system files outside
the home folder stay readable, since commands need them. Changes are allowed only in the
session's folder, temporary folders, the git folder its commits need, and the folders you allow
to change.

Let tools use their folders
: On by default. Commands may use the folders where tools like cargo, npm and git keep their
  programs, settings and caches. The **Tool folders** list starts with what this computer has: caches to
  write (`~/.cargo`, `~/.npm`, `~/.cache`, …), programs and settings to read (`~/.nvm`,
  `~/.local`, `~/.gitconfig`, …) and the `$PATH` folders in your home folder.

Allowed folders
: Folders outside the project the agent may read, or also write.

In both lists, tick **Read** and **Write** per folder, remove a folder with the trash can, and add
one with **+**.

Always hidden, also inside an allowed folder: `~/.ssh`, `~/.aws`, `~/.azure`,
`~/.config/gcloud`, `~/.gnupg`, `~/.config/gh`, `~/.docker/config.json`, `~/.kube`, `~/.netrc`,
`~/.git-credentials`, `~/.npmrc`, `~/.pypirc`, password stores, keychains and pi's `auth.json`.
Environment variables that hold keys and tokens (names ending in `_API_KEY`, `_TOKEN`,
`_SECRET`, …) are removed from commands. pi keeps them for its model requests.

## Internet

Allowed websites
: **None** (the default): a command reaches a website only once you allow it. The first time a
  conversation's commands use a site, you're asked in the conversation: **Allow** (for that
  conversation), **Always for this project**, or **Don't allow** (the command gets an HTTP 403).
  Connections made at once share one question.

  **All**: commands can reach every website.

  **Custom**: the websites listed reach through without asking; for others you're asked. The
  list starts with where code and packages come from (GitHub, npm, PyPI, crates.io, Go,
  RubyGems, Maven).

A website is allowed as a whole, since HTTPS doesn't show pages: `github.com`, or
`github.com/*` with its subdomains. The field checks an address as you type it and looks the
name up.

**Allowed when asked** lists the sites you allowed with "Always for this project", per project,
each removable.

None and Custom work through a proxy in Pistachio. Commands find it through `HTTPS_PROXY` and
the like, which curl, git, npm, pip and cargo use; tools that ignore those get no network. On
Linux, Landlock needs Linux 6.7 to limit the network.

## Git and GitHub

Your git and GitHub logins never enter the sandbox. Local git (commit, branch, merge, rebase,
log, …) runs in the sandbox as usual. When the agent needs your logins, you're asked, and it is
done outside the sandbox; the agent sees only what it prints. That covers:

- git's `clone`, `fetch`, `pull`, `push`, `ls-remote` and `submodule update`, and git-lfs;
- `gh` for a repository: reading pull requests, issues, runs and releases; opening, commenting
  on, closing or merging pull requests and issues; creating a repository, or deleting one
  (asked every time). `gh api`, logins and secrets are not run;
- signing a commit or tag with your GPG or SSH key.

The question is a card above the message box that says exactly what will happen: the commits,
the branch, the remote's real address, and whether it is a force push or a push to the default
branch. Answer **Allow**, **Always for this project** (for that project, that remote and that
kind of action) or **Don't allow**. With several GitHub accounts, the card offers each, and the
command runs with that account only.

The gate accepts only these commands and a list of their options, and only for remote servers.
git with your keys never works in the agent's repository: Pistachio keeps its own copy of each
remote, per project, out of the sandbox's reach. Fetches, pulls, clones and LFS downloads come in
through that copy, and pushes go out from it, with hooks off. So nothing the agent writes into its
repository's settings can run a program with your keys. If the repository changes while you're
asked, the command doesn't run. `gh pr create` needs `--title` and `--body` (or `--body-file`).

**Allowed with your keys** lists what you allowed with "Always for this project", per project,
each removable.

## Project files

A project's `.pi` folder can bring its own extensions (code that runs inside pi), MCP servers,
skills and settings. pi loads them only for projects you trust (on the Skills page, or `/trust`
in the terminal). `AGENTS.md` and `CLAUDE.md` are always read. Changes apply to sessions started
afterwards.

Without a decision
: pi's `defaultProjectTrust` setting: **Load only after I trust the project**, **Always load**
  or **Never load**.

Decisions
: The trust decisions saved in pi's `trust.json`, each removable. A decision covers the folder's
  subfolders.
