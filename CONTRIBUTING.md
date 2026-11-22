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
  sudo apt-get install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libvulkan-dev libx11-xcb-dev libxcb1-dev libfontconfig-dev libfreetype-dev libzstd-dev
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

## Handy tools

```bash
script/bundle-macos.sh [debug|release]   # wrap the binary in Pistachio.app (macOS)
script/shot.sh <folder> out.png terminal,diff   # relaunch the debug app and screenshot it
script/measure-memory.sh                 # memory of a running instance and its pi children
script/make-icon.sh                      # regenerate the .icns / PNG from icon.svg
```

`script/shot.sh` stops any running `Pistachio.app` first.

**Opening views at startup.** `PISTACHIO_OPEN` takes a comma-separated list, which makes layout
checks scriptable:

- Mode and panels: `chat`, `code`, `terminal`, `diff`, `files`, `no-sidebar`.
- Sessions and popovers: `latest`, `attach`, `context`.
- Dialogs: `add-host`, `manage-hosts`, `add-model`, `add-provider`, `edit-models`.
- Skills page: `skills`, `new-skill`, `open-skill:<path>`; the Settings page: `settings`.
- SSH: `ssh:<alias>`, `remote:<alias>:<folder>` and `remote-latest:<alias>` open a remote
  session once connected. Panels listed after them apply to that session.

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

`crates/transcript/tests/fixtures` holds sanitized recordings of real `pi --mode rpc`
sessions. The replay tests check that streaming and history render the same way.

## Reporting security issues

Please don't open public issues for vulnerabilities; see [SECURITY.md](SECURITY.md).

## License

By contributing you agree that your work is licensed under the project's dual
[MIT](LICENSE-MIT) / [Apache-2.0](LICENSE-APACHE) license.
