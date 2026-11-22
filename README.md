<p align="center">
  <img src="crates/app/assets/app-icon/icon-512.png" width="128" alt="Pistachio logo">
</p>

<h1 align="center">Pistachio</h1>

<p align="center">
  A fast, native desktop app for the <a href="https://pi.dev">pi coding agent</a>.
</p>

<p align="center">
  <a href="https://github.com/seba2390/pistachio/actions/workflows/ci.yml"><img src="https://github.com/seba2390/pistachio/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/seba2390/pistachio/releases"><img src="https://img.shields.io/github/v/release/seba2390/pistachio?include_prereleases" alt="Release"></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="License"></a>
</p>

<p align="center">
  <img src="docs/media/demo-hero.gif" width="900" alt="Pistachio demo">
</p>

Pistachio runs `pi --mode rpc` and draws pi's own event stream, so it behaves exactly like pi in
the terminal. Your sessions, models, skills and extensions come from your normal pi setup. It is
written in Rust and GPU-rendered with [GPUI](https://gpui.rs), and uses about 90 MB of memory
when idle.

## Features

- **Chat and Code modes.** Plain chat without tools, or the coding agent in a project folder.
- **A readable transcript.** Streaming markdown, collapsible thinking and inline edit diffs.
  Shell commands are labelled by what they do ("Read Cargo.toml", "Ran cargo test").
- **Side panels and a terminal** in the session's folder: git changes, a file tree, and an
  integrated terminal.
- **SSH remotes.** Run pi on another machine, and Pistachio installs pi there if it's missing.
- **Web search** for chats: quick lookups or multi-step deep research.
- **Background sessions.** Chats keep working while you switch, plus branching, model and
  skill management.
- **Settings and updates.** A Settings page (⌘,) for look, models, web search and SSH, and an
  optional check for new releases that asks before downloading.

See [docs/FEATURES.md](docs/FEATURES.md) for the full list and keyboard shortcuts.

## Demo

<!-- docs/media/demo.mp4, uploaded through GitHub (it only plays videos hosted there). After
re-recording it, upload the new file the same way and replace this link. -->
https://github.com/user-attachments/assets/45a10dcc-881a-451d-a78e-cefb881ec8d0


## Install

**1. Install pi** (needs Node.js 22.19+), then set it up as usual:

```bash
npm i -g @earendil-works/pi-coding-agent
```

**2. Install Pistachio**

- **macOS:** download `Pistachio-…-macos-universal.zip` from
  [Releases](https://github.com/seba2390/pistachio/releases), unzip it and move `Pistachio.app` to
  Applications.
  - The app isn't notarized yet, so macOS will block the first launch. Allow it once with:

    ```bash
    xattr -dr com.apple.quarantine /Applications/Pistachio.app
    ```

- **Linux (x86_64):** download `pistachio-…-linux-x86_64.tar.gz` from
  [Releases](https://github.com/seba2390/pistachio/releases), unpack it and run `./install.sh`.
  That installs for your user only and adds "Pistachio" to your app launcher.
- **Windows (x86_64, experimental):** download `pistachio-…-windows-x86_64.zip` from
  [Releases](https://github.com/seba2390/pistachio/releases), unzip it and run `pistachio.exe`.
  - Windows may warn about an unknown publisher the first time (More info → Run anyway).
  - SSH remotes are untested on Windows.
- **From source** (any platform, needs [Rust](https://rustup.rs)):

  ```bash
  cargo install --locked --git https://github.com/seba2390/pistachio pistachio
  ```

  Linux also needs the GPUI system libraries listed in [CONTRIBUTING.md](CONTRIBUTING.md#setup).

Then launch **Pistachio** (or run `pistachio`). If pi isn't on your `PATH`, point to it with
`PISTACHIO_PI_BIN=/path/to/pi`.

## Usage

Pick **Code** (⌘2) to work in a folder or **Chat** (⌘1) to just talk. The message box works
like pi's terminal:

| Key | Action |
|---|---|
| Enter / Shift+Enter | Send (steers while pi works) / new line |
| Alt+Enter | Queue a follow-up |
| Esc | Interrupt |
| `/` · `@` · `!` | Commands · mention a file · run a shell command |
| ⌘N · ⌘B · Ctrl+` | New session · sidebar · terminal |

## Contributing

Issues and pull requests are welcome. Start with [CONTRIBUTING.md](CONTRIBUTING.md), and see
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the code is organized. Please report
security issues privately ([SECURITY.md](SECURITY.md)).

## Acknowledgements

Pistachio is an independent project. It is not affiliated with or endorsed by the authors of pi.
It is built on:

- [pi](https://github.com/earendil-works/pi) by Mario Zechner (MIT), the agent this app is a
  window onto.
- [GPUI](https://gpui.rs) by Zed Industries (Apache-2.0) and
  [gpui-kit](https://github.com/longbridge/gpui-kit) by Longbridge (Apache-2.0).
- [alacritty_terminal](https://github.com/alacritty/alacritty) (Apache-2.0) for the
  integrated terminal, and [Lucide](https://lucide.dev) icons (ISC).

Release downloads include the full notices of every bundled dependency in
`THIRD-PARTY-LICENSES.txt`. You can generate the same file with
`script/third-party-licenses.py`.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
Unless you state otherwise, any contribution you submit is dual-licensed the same way, with no
extra terms.
