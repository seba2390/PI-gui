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
- Left sidebar: previous chats (also those started from the terminal), search, rename, trash.
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
| ⌘N / ⌘B / ⌘⌥B | New chat / toggle sidebar / toggle right panel |
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

`PI_GUI_OPEN=terminal,diff,files,no-sidebar` opens those panels at startup, which is what
`script/shot.sh` uses for layout checks without driving the mouse.

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

`crates/transcript/tests/fixtures` holds sanitized recordings of real `pi --mode rpc` sessions;
the replay tests check streaming and history rendering agree.
