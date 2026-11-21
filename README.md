# PI-gui

A lean, native desktop GUI for the [pi coding agent](https://pi.dev), written in Rust.

PI-gui is a **pure view over pi**: it runs `pi --mode rpc` as a child process and renders
pi's own event stream — thinking, tool calls, edits, retries, compaction — so behaviour stays
faithful to the terminal. Sessions are pi's session files; models, thinking levels, extensions,
skills and prompt templates all come from your normal pi configuration.

- Chat in the centre: streaming markdown, collapsible thinking (“Thought for 4s”), tool calls with
  results and inline edit diffs, and “Worked for 1m 3s” after every turn.
- Composer: attachments, working-folder picker, model and thinking pickers, context-window wheel,
  `/` command completion, `!cmd` shell commands.
- Left sidebar with a **Chat | Code** switch (⌘1 / ⌘2):
  - **Code**: the coding agent in a project folder; sessions are grouped under their folder
    (collapsible, "+" for a new session there), including ones started from the terminal.
  - **Chat**: plain conversation with **no tools**: the model cannot read, write or run
    anything. Chats run pi with `--no-tools` in a dedicated empty folder
    (`~/.pi/agent/pi-gui/chat`); pi records the empty tool set in the session and restores it
    on resume, so a chat stays tool-less even when reopened from the terminal. Text
    attachments are pasted into the prompt instead of being referenced by path, and `!`
    shell commands are disabled.
  - Search, rename and move to trash in both modes.
- **SSH** (Code mode): the device button next to the folder ("This computer", with a status
  light) runs pi on another machine. "Add SSH connection…" lists the hosts in your
  `~/.ssh/config` (following `Include`), or takes a custom host, user, port, key and jump host;
  connections are saved in `~/.pi/agent/pi-gui/hosts.json` and "Manage SSH connections…"
  renames them, changes the command that starts pi, copies your model settings and removes
  them. pi runs **on the remote** (its tools, `AGENTS.md`, skills and sessions all live there)
  through the system `ssh`, so your config, keys, ssh-agent and `known_hosts` apply.
  - One shared connection per host (ControlMaster) makes later commands ~30 ms. Saved hosts
    connect quietly in the background at startup; the dot shows green (ready), amber (working
    or needs you), red (failed) or grey (not connected).
  - **pi is installed automatically** when a host does not have it, like an editor's remote
    server: into `~/.pi-gui/server` on the host, with no root access, using the host's
    Node.js 22.19+ or a downloaded, checksum-verified standalone Node.js. Remove that folder
    to undo it. If the host has no model settings, the connect dialog offers to copy your
    `models.json` (credentials in `auth.json` only if you tick the box; an existing file is
    kept as `*.bak-pi-gui`).
  - Passwords, key passphrases and one-time codes are asked for in the app (ssh's
    `SSH_ASKPASS`); nothing is stored. Unknown host keys are shown for confirmation; a changed
    host key is explained, and you can remove the old key and confirm the new one.
  - The folder browser takes a typed or pasted path, creates folders and can show hidden ones.
  - Remote sessions behave like local ones: they are listed in the sidebar under the host
    (then folder), can be opened, renamed and moved to the host's trash, and the last one
    comes back after a restart. The Diff and Files tabs read git and the folder over SSH
    (refreshed every 10 s while shown and after each turn), the terminal is your shell on the
    host in the session's folder, and the attach picker indexes the remote folder.
  - When the connection drops, the session offers to reconnect and resumes its session file.
  - Limits: csh/tcsh login shells are not supported (bash, zsh, dash, ksh and fish are; fish
    is untested on a real host). Windows clients are untested: the `remote` crate uses Unix
    sockets, and Windows' OpenSSH cannot share connections.
- Right panel: git changes and a file tree of the working folder.
- Bottom panel: an integrated terminal in the working folder.
- macOS and Linux (Windows best-effort). GPU-rendered with [GPUI](https://gpui.rs) via
  [gpui-kit](https://gpui-kit.com).

## Requirements

- `pi` on your `PATH` (`npm i -g @earendil-works/pi-coding-agent`), configured as usual.
  Override the executable with `PI_GUI_PI_BIN=/path/to/pi`.
- Rust (stable, see `rust-toolchain.toml`).
- macOS: Xcode command line tools. Linux: `libxkbcommon`, `wayland`, `vulkan`, `fontconfig`
  development packages (see `.github/workflows/ci.yml`).

## Run

```bash
cargo run --release
```

On macOS, `script/bundle-macos.sh [debug|release]` wraps the binary in `PI.app`.

## Keyboard

Composer behaviour mirrors pi's terminal.

| Key | Action |
|---|---|
| Enter / Shift+Enter | Send (steers while the agent works) / newline |
| Alt+Enter | Queue a follow-up message |
| Esc | Interrupt (queued messages return to the editor) |
| Alt+↑ | Restore queued messages to the editor |
| Shift+Tab | Cycle thinking level |
| Tab | Complete a `/` command |
| Ctrl+T / Ctrl+O | Expand all thinking / tool output |
| ⌘N / ⌘B / ⌘⌥B | New chat or session / toggle sidebar / toggle right panel |
| ⌘1 / ⌘2 | Chat mode / Code mode |
| ⌘⇧D / ⌘⇧E | Changes / files panel |
| Ctrl+` | Toggle terminal |

(Use Ctrl instead of ⌘ on Linux/Windows.)

Built-in pi commands that the TUI handles itself are mapped onto RPC: `/new`, `/compact`, `/name`,
`/model`, `/thinking`, `/session`, `/copy`, `/quit`. Extension commands, prompt templates and
`/skill:…` go to pi unchanged.

## Architecture

```
crates/
  pi-rpc/       typed protocol (serde mirror of pi's rpc-types), strict JSONL framing, client, process
  transcript/   pure reducer: pi events / stored messages → renderable transcript (no UI, no I/O)
  pi-sessions/  read-only index of pi's session files for the sidebar
  workspace/    git status/diff (git CLI) + gitignore-aware file listing + fs watcher
  remote/       SSH: ~/.ssh/config discovery, saved hosts, shared connections, remote probes,
                pi auto-install, askpass prompts, remote sessions, the RemoteShell for workspace
  term/         terminal backend: alacritty_terminal + PTY, UI-agnostic snapshots
  app/          the GPUI application (binary `pi-gui`)
```

Only `app` depends on the UI toolkit; everything else is plain Rust with its own tests. The app
keeps a pi process alive only for the visible chat and for chats whose agent is still working.

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
script/measure-memory.sh          # RSS / footprint of a running instance and its pi children
script/bundle-macos.sh && script/shot.sh <folder> out.png terminal,diff   # scripted screenshot (macOS)
```

`PI_GUI_OPEN=chat|code,terminal,diff,files,no-sidebar,latest,attach,context,add-host,manage-hosts,ssh:<alias>`
opens those panels (or the newest session, the attach picker, the context breakdown) at
startup, which is what `script/shot.sh` uses for layout checks without driving the mouse.
`remote:<alias>:<folder>` (a new session there) and `remote-latest:<alias>` (the host's newest
session) open a remote session once connected; panels listed after them apply to it.

The app icon is `crates/app/assets/app-icon/icon.svg`; `script/make-icon.sh` regenerates the
`.icns` (and a 512 px PNG for Linux launchers) from it.

### Performance notes

- Each column (sidebar, chat, right panel, terminal) is a cached GPUI view: it re-renders only
  when its own state changes, so a streaming answer does not re-render the sidebar or terminal.
- Streaming text is appended to retained markdown state (no full re-parse or copy per delta);
  pi events are drained in batches of up to 512 per UI update.
- Transcript, diff and file lists are virtualized; git, directory listing and session scans run
  on background threads, and session rescans re-read only files whose mtime changed.
- Measured on macOS (release build, ~2700×1750 Retina window): 89 MB physical footprint idle,
  96 MB with the terminal and Diff panel open; the app's own heap is ~9 MB, the rest is GPU
  drawable surfaces. Each live pi process adds ~110 MB (Node), so only the visible chat and
  still-working chats keep one.
- The terminal repaints only on the emulator's coalesced wakeups and copies just the visible
  viewport into a reused buffer; a hidden terminal costs nothing.

SSH tests against a real host (all `--ignored`, run with `PI_GUI_SSH_TEST_HOST=<alias>`):
`cargo test -p remote --test live -- --ignored` (connection, probe, quoting, remote git /
files / sessions / trash in a scratch folder, and `live_install`, which installs pi on the
host), and `cargo test -p pi-gui -- --ignored` (RPC over SSH with a scripted fake pi, and
`real_pi_over_ssh`: real pi started the way a remote session is, a shell command through pi, a
prompt turn, the session listed from the host and resumed). `askpass_prompt_reaches_the_app`
needs `PI_GUI_SSH_PASSWORD_HOST`, a host that offers password login (it cancels the prompt).

`crates/transcript/tests/fixtures` holds sanitized recordings of real `pi --mode rpc` sessions;
the replay tests check streaming and history rendering agree.
