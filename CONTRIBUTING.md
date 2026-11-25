# Contributing to Pistachio

Thanks for helping! Bug reports, ideas and pull requests are all welcome.

## Ground rules

- **Pistachio is a view over pi.** Agent behaviour belongs in pi, not here: the app renders pi's
  RPC events and forwards your input. If a change re-implements something pi does, it
  probably belongs upstream.
- **Keep the UI crate thin.** Logic goes into the UI-agnostic crates (`transcript`,
  `workspace`, `remote`, …) with unit tests; `crates/app` wires it to GPUI. See
  [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
- **Stay lean.** Memory and idle CPU matter: no polling loops, no work for hidden views.
- Open an issue first for anything large, so we can agree on the approach.

## Setup

You need stable Rust (see `rust-toolchain.toml`) and [pi](https://pi.dev) on your `PATH`.

- **macOS:** Xcode command line tools (`xcode-select --install`).
- **Linux (Debian/Ubuntu):**

  ```bash
  sudo apt-get install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan-dev libx11-xcb-dev libxcb1-dev libfontconfig-dev libfreetype-dev libzstd-dev libasound2-dev
  ```

Then:

```bash
cargo run            # debug build
cargo run --release  # what users get
```

## Before you open a pull request

CI runs exactly these; please run them locally:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- Add or update tests for logic changes (UI-agnostic crates make this easy).
- Keep commits focused, with messages that say *why*.
- Update [docs/FEATURES.md](docs/FEATURES.md) and [CHANGELOG.md](CHANGELOG.md) for
  user-visible changes.

## Releasing

For maintainers. Describe the changes under `## [Unreleased]` in [CHANGELOG.md](CHANGELOG.md) (it
becomes the release notes), commit everything, and run this on the branch with the changes:

```bash
script/release.sh 0.5.0   # --dry-run checks everything and changes nothing
```

It runs CI's checks, sets the version, dates the changelog section, pushes, opens a pull request
into `main` and turns on auto-merge. Then GitHub does the rest: it merges once the required checks
pass, `tag-release.yml` tags the merge and starts `release.yml`, and that builds the downloads,
publishes the release and updates the Homebrew cask. If a required check fails, nothing is
released: push a fix to the branch. If `release.yml` fails, run it again on the tag (Actions →
Release → Run workflow).

## Handy tools

```bash
script/bundle-macos.sh [debug|release]   # wrap the binary in Pistachio.app (macOS)
script/shot.sh <folder> out.png terminal,diff   # relaunch the debug app and screenshot it
script/measure-memory.sh                 # memory of a running instance and its pi children
script/perf.sh [release|debug] [runs] [baseline.json]   # time startup, navigation and quitting
script/check-linux-displays.sh <app> <folder>  # that the app opens on X11 and on Wayland (Linux)
script/make-icon.sh                      # regenerate the .icns / PNG from icon.svg
script/set-version.sh 0.5.0              # bump the version (Cargo.toml, Cargo.lock, README install links)
script/update-searxng.sh                 # move the pins of the SearXNG the app can download
script/update-parsers.sh                 # move the pins of the document parsers the app can download
script/release.sh 0.5.0                  # release a version (see Releasing)
```

`script/shot.sh` stops any running `Pistachio.app` first.

**Opening views at startup.** `PISTACHIO_OPEN` takes a comma-separated list, which makes layout
checks scriptable:

- Mode and panels: `chat`, `code`, `terminal`, `diff`, `files`, `no-sidebar`;
  `diff-expand-all` opens the Changes tab with every file expanded, and `expand-tools` expands
  every tool call in the chat (like Ctrl+O); `view-file:<path>` opens a file (relative to the
  folder) in the Files tab's viewer.
- Sessions and popovers: `latest`, `new-chat` (like ⌘N), `attach`, `context`; `find:<text>` opens the find bar
  searching for `<text>`; `attach:<path>` attaches a file and `send:<text>` sends `<text>` as a
  message, once what is attached is read; `image-viewer:<n>` opens the picture viewer on the session's
  `<n>`th picture; `side-chat` opens a side chat of it 2 s after start; `report-check` opens how the session's last deep search report was checked.
- Dialogs: `add-host`, `manage-hosts`, `add-model`, `add-provider`, `edit-models`.
- Skills page: `skills`, `new-skill`, `open-skill:<path>`; the Settings page: `settings`
  (`settings-web`, `settings-speech`, `settings-names` and `settings-documents` open those pages; after `settings-web`,
  `web-advanced` opens its advanced sections,
  `searxng-setup` asks whether to download SearXNG and `searxng-get` downloads and runs it); `type-name:<text>` plays the
  sidebar's typing animation for the open session 2.5 s after start (nothing is saved).
- SSH: `ssh:<alias>`, `remote:<alias>:<folder>` and `remote-latest:<alias>` open a remote
  session once connected. Panels listed after them apply to that session.

**Response times.** `script/perf.sh` builds the app and runs a fixed navigation script
(`PISTACHIO_BENCH=1`, see `crates/app/src/perf/bench.rs`) against generated sessions in a
private pi folder, then prints the median time from each input to the frame that shows its
response on screen, plus startup phases, the time to quit and render times per view. Pass the
`summary.json` of an earlier run to compare. The window takes focus for about 20 s per run, and
each run moves one generated session to the Trash. `PISTACHIO_PERF=<file.jsonl>` records the
same measurements in a normal session.

**Update prompt.** In debug builds, `PISTACHIO_PRETEND_VERSION=0.0.1` makes "Check for
Updates…" treat the latest release as new.

**Separate state.** Setting `PI_CODING_AGENT_DIR` to a scratch folder runs the app with its
own state, so it won't touch your normal pi sessions or settings.

**The README demo.** `script/demo/record-demo.sh` re-records `docs/media/demo.mp4` after UI
changes. It needs [OpenScreen](https://getopenscreen.com) (`brew install --cask openscreen`) and
`ffmpeg` (`gifski` too for `post.sh --gif`, a short looping GIF), plus the model named in
`script/demo/readme-demo.toml`. It sets up an isolated agent dir and a demo project in `/tmp`,
plays the storyboard in the app's demo mode (`PISTACHIO_DEMO=<storyboard.toml>`, see
`crates/app/src/demo`) while OpenScreen records the window, then edits the take
(`script/demo/post.py`). Leave the mouse and keyboard alone while it records. The steps also run
on their own: `setup.sh`, `record.sh` and `post.sh` in `script/demo/`. GitHub only plays videos
uploaded to it, so after re-recording, drag the new `demo.mp4` into a comment box on github.com
(without posting) and put the link it creates at the top of the README.

## Tests against real hosts

These tests are all `#[ignore]`d. Run them with `PISTACHIO_SSH_TEST_HOST=<alias>`:

- `cargo test -p remote --test live -- --ignored`: connection, probe, quoting, remote git,
  files, sessions and trash in a scratch folder.
  - `live_install` installs pi on the host, so it writes to the host.
- `cargo test -p pistachio -- --ignored`: RPC over SSH with a scripted fake pi.
  - `real_pi_over_ssh` starts real pi the way a remote session does.
  - `askpass_prompt_reaches_the_app` also needs `PISTACHIO_SSH_PASSWORD_HOST`, a host that
    offers password login.

`PISTACHIO_PREVIEW_CHECK=<folder of session copies> cargo test -p pistachio preview_matches_pi --
--ignored` runs real pi on each session file and checks that the history Pistachio previews
from the file matches pi's (use copies: pi may migrate files it opens).

`script/test-deepsearch.py` runs deep search's extension in real pi, with a scripted model, search
service and pages on this computer (`cargo build` first). It covers what no unit test can: sending
the model back, revisions, a model that does not answer, Esc and more.

`cargo test -p documents --test live -- --ignored --test-threads 1` really downloads the document
parsers (2 GB, into a temporary folder) and reads a PDF with each; add `pymupdf4llm`, `docling` or
`mineru` for one of them.

`TITLES_PI_MODEL=<provider/model> cargo test -p titles real_pi -- --ignored --nocapture` names
a few sample first messages with real pi and that model (the conversation-model namer).

`crates/transcript/tests/fixtures` holds sanitized recordings of real `pi --mode rpc`
sessions. The replay tests check that streaming and history render the same way.

## Reporting security issues

Please don't open public issues for vulnerabilities; see [SECURITY.md](SECURITY.md).

## License

By contributing you agree that your work is licensed under the project's dual
[MIT](LICENSE-MIT) / [Apache-2.0](LICENSE-APACHE) license.
