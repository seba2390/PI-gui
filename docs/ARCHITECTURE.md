# Architecture

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
  titles/       automatic conversation names: title clean-up, the one-shot `pi --print` call, and
                small local models (quantized GGUF via candle, pure Rust)
  deepsearch/   chat web search (search / deep): search backends, public-only fetching, page text extraction,
                PDF text, passage ranking, checking reports, settings, and the pi extension that runs them via the app
  searxng/      a private SearXNG for that: downloading it on request (pinned and checked), running it,
                checking one at an address
  app/          the GPUI application (binary `pistachio`)
```

Only `app` depends on the UI toolkit; everything else is plain Rust with its own tests. The app
keeps a pi process alive only for the visible chat and for chats whose agent is still working.

## Performance

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
