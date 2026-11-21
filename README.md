# PI-gui

A lean, native desktop GUI for the [pi coding agent](https://pi.dev), written in Rust.

PI-gui is a **pure view over pi**: it runs `pi --mode rpc` as a child process and renders
pi's own event stream — thinking, tool calls, edits, retries, compaction — so behaviour stays
faithful to the terminal. Sessions are pi's session files; models, thinking levels, extensions,
skills and prompt templates all come from your normal pi configuration.

- Chat in the centre: streaming markdown, collapsible thinking (“Thought for 4s”), tool calls with
  results and inline edit diffs, and “Worked for 1m 3s” after every turn.
- Bash calls read as what they do (“Read Cargo.toml, src/main.rs”, “Searched “fn main” in src”,
  “Ran cargo test”), worked out by the app from the command itself; the model writes nothing
  extra. Runs of calls that only look around (reading, listing, searching, checking) fold into
  one “Explored · N steps” row; expanding it shows every call. Anything not fully understood
  (loops, `$(…)`, `xargs`, unknown programs, flags that could change files such as
  `find -delete` or `sed -i`) is shown as the raw command, so a label never hides a change. A
  labelled call shows its command when expanded. To check the labeller against real
  transcripts: `cargo run --release -p transcript --example label_corpus -- <session.jsonl…>`.
- Composer: attachments, working-folder picker, model and thinking pickers, context-window wheel,
  `/` command completion, `!cmd` shell commands.
- Left sidebar with a **Chat | Code** switch (⌘1 / ⌘2):
  - **Code**: the coding agent in a project folder; sessions are grouped under their folder
    (collapsible, "+" for a new session there), including ones started from the terminal.
  - **Chat**: plain conversation with **no file or shell tools**: the model cannot read, write
    or run anything on your computer. Chats run pi in a dedicated empty folder
    (`~/.pi/agent/pi-gui/chat`) with only deep search's two web tools (`--tools
    web_search,web_fetch --no-mcp`), which stay off until you turn web search on. Text
    attachments are pasted into the prompt instead of being referenced by path, and `!`
    shell commands are disabled.
  - **Web search** (Chat): the globe button next to the paperclip chooses how the model uses
    the web, per chat (remembered in the session; `/websearch off|search|deep` works too):
    - **Search**: a quick lookup when needed (a few searches and pages), then a concise answer
      with links.
    - **Deep search**: a thorough investigation: the model plans sub-questions, searches from
      several angles, reads many pages, cross-checks key facts and reports back with a
      structured, sourced report. Takes minutes rather than seconds.

    The model searches (`web_search`) and reads pages (`web_fetch`); the work is done in Rust
    by the app itself, and a small embedded pi extension only connects the tools to pi. Pages
    are read without JavaScript, reduced to their main text, and long pages are cut down to
    the passages that match what the model is looking for. Only public addresses are fetched
    (no localhost or private networks), and each message has a limit on searches and page
    reads. Settings live in `~/.pi/agent/pi-gui/deepsearch/` and apply to the next message,
    no restart:
    - `config.json`: `backend` (`duckduckgo` by default, no key; `searxng` with
      `searxng.url`, e.g. a self-hosted instance with `json` enabled in its `settings.yml`;
      `brave` or `tavily` with an API key or `api_key_env`), `default_mode` for new chats,
      `modes.search` / `modes.deep` limits (`max_searches`, `max_fetches` per message),
      results per search, page text per read (`fetch.max_chars`; lower it for small context
      windows), `fetch.allow_private_networks` and timeouts. Settings added in later versions
      are filled in automatically.
    - `prompt-search.md` and `prompt-deep.md`: each mode's instructions (`{{date}}`,
      `{{max_searches}}` and `{{max_fetches}}` are filled in). Delete a file to get the
      default back.
    - Try settings without the GUI: `pi-gui --deepsearch-tool search <query>` and
      `pi-gui --deepsearch-tool fetch <url> [what to look for]` print what the model would
      see.
  - Search, rename and move to trash in both modes.
  - **Background work**: switching away from a chat that is working (or still has messages
    waiting to be sent) keeps it running. Its row shows a spinner; when it finishes it gets a
    green dot until you open it, a toast says so, and the Dock icon bounces if the app is not
    in front. Questions from extensions (confirm, select, input) in a background chat wait
    until you open that chat, marked with an amber dot, so they are never answered for the
    wrong chat; its toasts name the chat.
- **Branches**: hovering a prompt offers Copy and Edit; hovering the end of a turn offers
  Copy reply, Retry (newest turn) and Branch from here. Edit, Retry and branching continue in
  a new session (the original stays in the sidebar), like pi's `/fork`. Slash commands:
  `/fork` (pick a message), `/clone`, `/resume` (pick an earlier session here), `/export
  [path]` (HTML) and `/share` (a secret GitHub gist via `gh`, like pi's terminal).
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
- **Skills** (Code mode): a Skills button under the Chat | Code switch shows the skills pi loaded
  for the session (personal ones from `~/.pi/agent/skills` and `~/.agents/skills`, project ones
  from `.pi/skills` and `.agents/skills`), as pi itself reports them, here or on the SSH host.
  "New skill" writes a `SKILL.md` (name checked against pi's rules, description, instructions,
  optionally "only when I call it" via `disable-model-invocation`) into your personal skills or
  the project's `.pi/skills`; existing skills open in an editor, with the toggle and move to
  trash. pi loads project resources only for trusted folders, and in RPC mode it cannot ask, so
  the page shows whether the project is trusted and can trust it, saved in pi's own
  `trust.json` like the terminal's `/trust`. pi reads skills when it starts, so changes restart
  it on the same session.
- **Add model**: the model picker under the composer ends with "Add model…" and "Edit
  models.json…". The form adds a custom model to pi's `models.json`, to a provider already in
  the file or a new one (presets for Ollama, LM Studio, vLLM, llama.cpp and OpenRouter; any
  OpenAI-, Anthropic- or Gemini-compatible endpoint), and can list the server's models to pick
  from. The other tab edits the file directly, with validation. It is the file the session's pi
  reads (on the SSH host for remote sessions); the previous version is kept as
  `models.json.bak-pi-gui`. pi reads the file only when it starts, so after saving it restarts
  on the same session (after the current turn, if one is running) and selects the new model.
- Right panel: git changes and a file tree of the working folder.
- Bottom panel: an integrated terminal in the working folder; drag its top edge to resize it
  (the height is remembered).
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
  pi-config/    editing pi's config: models.json, SKILL.md front matter, the trust store
  workspace/    git status/diff (git CLI) + gitignore-aware file listing + fs watcher
  remote/       SSH: ~/.ssh/config discovery, saved hosts, shared connections, remote probes,
                pi auto-install, askpass prompts, remote sessions, the RemoteShell for workspace
  term/         terminal backend: alacritty_terminal + PTY, UI-agnostic snapshots
  deepsearch/   chat web search (search / deep): search backends, public-only fetching, page text extraction,
                passage ranking, settings, and the pi extension that runs them via the app
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
`add-model`, `add-provider` and `edit-models` open the models dialog; `skills`, `new-skill`
and `open-skill:<path>` the Skills page.
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
