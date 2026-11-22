# Changelog

All notable changes to PI-gui are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/seba2390/pistachio/compare/v0.4.1...HEAD
[0.4.1]: https://github.com/seba2390/pistachio/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/seba2390/pistachio/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/seba2390/pistachio/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/seba2390/pistachio/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/seba2390/pistachio/releases/tag/v0.1.0
