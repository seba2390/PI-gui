# Pistachio

**A native desktop app for the [pi coding agent](https://pi.dev).** Pistachio runs
`pi --mode rpc` and draws pi's own event stream, so it behaves like pi in the terminal: your
sessions, models, skills and extensions come from your normal pi setup. It is written in Rust,
GPU-rendered with [GPUI](https://gpui.rs), and runs on macOS, Linux and (experimentally)
Windows.

<video src="_static/demo.mp4" controls muted playsinline preload="metadata" width="100%"></video>

## Install

**1. Install pi** (needs Node.js 22.19 or newer), then set it up as usual:

```bash
npm i -g @earendil-works/pi-coding-agent
```

**2. Install Pistachio.**

macOS (Apple Silicon)
: With [Homebrew](https://brew.sh):

  ```bash
  brew install --cask seba2390/tap/pistachio
  ```

  Without Homebrew, download `Pistachio-…-macos-arm64.zip` from the
  [latest release](https://github.com/seba2390/pistachio/releases/latest), unzip it and move
  `Pistachio.app` to Applications. The app isn't notarized yet, so allow the first launch with:

  ```bash
  xattr -dr com.apple.quarantine /Applications/Pistachio.app
  ```

Linux (x86_64 and ARM64)
: Download the package for your system from the
  [latest release](https://github.com/seba2390/pistachio/releases/latest) and install it:

  ```bash
  sudo apt install ./pistachio_*.deb      # Debian, Ubuntu
  sudo dnf install ./pistachio-*.rpm      # Fedora and other RPM-based systems
  ```

  On any other distribution, unpack `pistachio-v…-linux-<arch>.tar.gz` and run the
  `install.sh` inside it. It installs for your user only (no root) and adds Pistachio to your
  app launcher. The [README](https://github.com/seba2390/pistachio#install) has one-line
  commands for each.

Windows (x86_64, experimental)
: Download `pistachio-…-windows-x86_64.zip` from the
  [latest release](https://github.com/seba2390/pistachio/releases/latest), unzip it and run
  `pistachio.exe`. Windows may warn about an unknown publisher the first time
  (More info → Run anyway). SSH remotes are untested on Windows, and there is no sandbox yet.

From source
: Needs [Rust](https://rustup.rs). Linux also needs the GPUI system libraries listed in
  [CONTRIBUTING.md](https://github.com/seba2390/pistachio/blob/main/CONTRIBUTING.md#setup).

  ```bash
  cargo install --locked --git https://github.com/seba2390/pistachio pistachio
  ```

Then launch **Pistachio** (or run `pistachio`). If pi isn't on your `PATH`, set it in
[Settings → pi & models](settings/pi-and-models.md), or start the app with
`PISTACHIO_PI_BIN=/path/to/pi`.

Pistachio offers new versions itself: when one is out, **Update** installs it the way you
installed the app (see [Updates](general.md#updates)).

## First steps

1. Pick **Chat** (⌘1) to talk, or **Code** (⌘2) to have the agent work in a project folder.
2. Press ⌘N for a new conversation. In Code mode you choose the folder, on this computer or an
   SSH host.
3. Type and press Enter. The message box works like pi's terminal: `/` for commands, `@` to
   mention a file, `!` to run a shell command.

[General](general.md) walks through each part of the app. The Settings pages (⌘,) each have a
page here.

On Linux and Windows, use Ctrl where these pages say ⌘.

```{toctree}
:hidden:

Home <self>
general
```

```{toctree}
:caption: Settings
:hidden:

settings/pi-and-models
settings/web-search
settings/ssh-connections
settings/speech-to-text
settings/conversation-names
settings/document-parsing
settings/sandboxing-and-security
settings/tools
```

```{toctree}
:caption: Project
:hidden:

GitHub <https://github.com/seba2390/pistachio>
Releases <https://github.com/seba2390/pistachio/releases>
Changelog <https://github.com/seba2390/pistachio/blob/main/CHANGELOG.md>
Contributing <https://github.com/seba2390/pistachio/blob/main/CONTRIBUTING.md>
```
