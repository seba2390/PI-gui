# Changelog

All notable changes to PI-gui are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/seba2390/PI-gui/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/seba2390/PI-gui/releases/tag/v0.1.0
