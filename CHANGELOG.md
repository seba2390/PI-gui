# Changelog

All notable changes to PI-gui are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Pictures in the chat: when the agent reads an image (a screenshot, say), a small thumbnail
  shows to the right of the call, and an "Explored" group shows the first two with "+N".
  Pictures attached to a prompt show above it. Clicking one opens a full-window viewer; ← / →
  or the arrows browse all the pictures of that call, group or prompt, and Esc closes it.

### Changed

- The model's text stands apart from its activity: runs of tool calls and thinking sit on a
  thin rail on the left, in a slightly smaller, muted style, with more room where they meet
  the text. An image read says "Read an image" instead of "Read 1 line".

- Tool calls in the chat read in the past tense like the rest: "Wrote", "Edited", "Listed" and
  "Searched" instead of "Write", "Edit", "List" and "Search".

## [0.4.2] - 2026-11-22

### Added

- Speech to text: dictate with the microphone button in the message box. The text appears
  greyed out while you speak and becomes normal text when you stop. A model runs on your
  computer (Parakeet v3, Whisper large-v3 turbo, Whisper small or Moonshine v2); none ship
  with the app. The first press offers to download one, with its size, speed and languages;
  it downloads in the background with a progress bar at the top of the chat, and is checked
  against its SHA-256. Settings → Speech to text manages the models, when a silence ends
  dictation and the language for Whisper.
- Dictation shortcut: ⌘⇧Space (Ctrl+Shift+Space on Linux/Windows) in the message box. Hold
  it to talk and let go to keep the text, or tap it to start and tap again to stop. Without a
  model it offers the download, like the microphone button.
- Find in chat (⌘F, also Edit → Find in Chat): highlights every match in the conversation,
  with Enter / Shift+Enter (or ⌘G / ⌘⇧G) for the next / previous one and a "3 of 12" count;
  Esc closes it. Thinking, tool output and edits are searched too, and a collapsed row opens
  when it holds the current match.
- File viewer: clicking a file in the Files tab opens it read-only in the panel, with syntax
  colors and line numbers, also for folders on an SSH host. "Mention" in the viewer, the +
  button on a row, or ⌥-click in the tree still put an `@path` mention in the message box.
  Binary files show a notice; files over 512 KB open with their first 512 KB.
- Settings → Appearance → Code colors: pick the syntax colors for code (Pistachio, One,
  GitHub, Solarized, Gruvbox or Catppuccin), each with a light and a dark variant. A Python
  snippet below the picker shows the chosen palette right away. Only code colors change; the
  rest of the app keeps its theme.

### Changed

- Clicking a file in the Files tab opens it instead of mentioning it (⌥-click mentions it).
- The models.json editor colors JSON: keys, strings, numbers, `true`/`false` and `null` each
  get their own color.
- The skill editor colors Markdown: headings, bold and italic, code, links and list markers.
- Code blocks in chat are colored for Python, Rust, JavaScript, TypeScript (and TSX), C, C++,
  C#, Go, Java, HTML, CSS, Bash, YAML, TOML, JSON, Markdown and diffs. Other languages
  still get the labeled box with a copy button, in one color.
- The agent's file edits in the chat, the files it reads, and the diffs in the Changes panel
  are colored the same way (by the file's extension), on top of the red and green
  added/removed backgrounds. Each side of a diff is colored as a whole, so a string or comment
  spanning several lines stays colored. They follow the Code colors setting.

### Fixed

- New session / New chat (and ⌘N) did nothing while the Skills or Settings page was open over a
  conversation that was still empty; it now closes the page and shows that conversation.
- Esc in the message box now interrupts the agent: the text box kept the key to itself.
- Dragging the right panel wide no longer squeezes the chat until its header buttons and the
  message box controls overlap: the chat keeps at least 400 px, the title truncates, and in a
  narrow column the left side of the message box toolbar is clipped while the model picker,
  thinking level and send / stop button stay visible.

## [0.4.1] - 2026-11-22

### Added

- Linux builds for ARM64 (aarch64), and `.deb` and `.rpm` packages for x86_64 and ARM64.
- Install on macOS with Homebrew: `brew install --cask seba2390/tap/pistachio`.

### Fixed

- "Check for Updates…" on Linux offers the download for your architecture.

## [0.4.0] - 2026-11-22

### Added

- New chats and Code sessions show the π field behind their empty screen; it fades out when
  the first message is sent.
- A demo video in the README, recorded with the new scripted demo mode
  (`PISTACHIO_DEMO=<storyboard.toml>`; re-record with `script/demo/record-demo.sh`).

### Fixed

- Debug builds no longer panic when a run of exploration steps ("Explored · N steps") appears
  in the chat.

## [0.3.0] - 2026-11-22

### Added

- **Settings page** (⌘, or the button at the bottom of the sidebar): theme, text size, start
  mode, reopening the last session, background notifications, which `pi` to run, pi's startup
  model and thinking level, web search service, keys and limits, and SSH connections.
- **Check for Updates…** in the Pistachio menu and in Settings, plus an optional check when the
  app opens. A newer release is offered (what's new, download in the browser, skip this
  version); nothing is downloaded without a yes.

### Changed

- The last session is reopened at start only with the new "Reopen the last session" setting
  (before, an SSH session came back on its own).
- Saved SSH connections connect at start only with the new "Connect saved connections at
  start" setting (off by default; before, they always did).
- The sidebar's search field matches the Settings page's (search icon, regular size), and
  switches in Settings show a hover effect.
- Web search's `config.json` is written atomically and readable only by you.

### Fixed

- The Windows release packaging (license notices with non-ASCII names), so 0.2.0's changes
  ship in this release.

## [0.2.0] - 2026-11-22

Tagged but not published as downloads (the Windows packaging failed); its changes are in
0.3.0.

### Changed

- **PI-gui is now Pistachio.** The app, binary (`pistachio`), macOS bundle (`Pistachio.app`,
  `dev.pistachio.app`), Linux launcher and repository (`seba2390/pistachio`) are renamed.
  - Settings move on first start from `~/.pi/agent/pi-gui/` to `~/.pi/agent/pistachio/`.
    Earlier chats stay where they are and still show up under Chat.
  - On SSH hosts pi is now installed into `~/.pistachio/server`; an existing
    `~/.pi-gui/server` is still used until then.
  - Environment variables are now `PISTACHIO_*` (for example `PISTACHIO_PI_BIN`,
    `PISTACHIO_OPEN`).

### Added

- Windows build (experimental): a `pi-gui.exe` zip in each release. pi installed with npm
  (`pi.cmd`) is found on `PATH`. CI builds and lints Windows; SSH remotes are untested there.

### Changed

- CodeQL runs from a workflow on `main` and weekly instead of on pull requests.

## [0.1.0] - 2026-11-22

The first public release: chat and code sessions over pi's RPC mode, with streaming
transcripts, git changes and file panels, an integrated terminal, SSH remotes, web search for
chats, skills and model management. Changes since the last private build:

### Added

- Labelled bash calls show only their label; click to see the command and output.
- Nested tool activity (subagents, code mode) shows inside its parent tool call.
- Composer: ↑ / ↓ prompt history, inline `@file` completion, and a "Waiting to send" list with
  cancel buttons.
- **Restart pi** for local sessions whose pi stopped.
- Accessibility: names and roles for icon buttons, sidebar rows, the Chat | Code switch,
  expandable transcript rows and the message box.
- Window menu (Minimize, Zoom), ⌘W and ⌘M. Closing the window hides the app (macOS), and the
  Dock icon brings it back.
- Terminal: each folder keeps its own shell. IME composition is drawn. On macOS, Option-arrows
  move by word and ⌘-arrows / ⌘-Backspace edit the line.

### Fixed

**Restarts and background chats**

- Restarts after saving `models.json` or a skill now apply to the chat that asked for them.
  They also wait for prompts that are still being sent.
- Closing the Skills page no longer keeps its pi process running.
- Prompts typed while pi wasn't ready are no longer lost silently.
- Esc and Alt+↑ merge restored messages into your draft instead of replacing it.
- Retry and Edit keep the prompt's images.
- A background chat whose pi crashed is reported as stopped, not "done".
- Questions held for a background chat are dropped once pi has timed them out.
- `/clone` waits for the current response.
- Startup notices survive resuming a session.
- A pi that hangs on exit is stopped together with the processes its tools started.

**SSH and security**

- The host key you confirm is exactly the key that gets saved: the connection is pinned to it.
  "Trust" is no longer offered when the key couldn't be fetched.
- The SSH connection and password-prompt folders are refused unless they are private to you.
- Saving `models.json` keeps its file permissions.
- The remote probe can no longer mistake login-shell output for pi's folders.
- Host names starting with `-` are rejected.

**Performance and saved state**

- Per-file diffs are capped at 2 MB, and line counts of untracked files at 500 files.
- Saves of UI state and SSH hosts can no longer land out of order.
- A corrupt `hosts.json` is kept as a backup.
- A remembered terminal height can no longer squeeze out the chat.

[Unreleased]: https://github.com/seba2390/pistachio/compare/v0.4.2...HEAD
[0.4.2]: https://github.com/seba2390/pistachio/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/seba2390/pistachio/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/seba2390/pistachio/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/seba2390/pistachio/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/seba2390/pistachio/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/seba2390/pistachio/releases/tag/v0.1.0
