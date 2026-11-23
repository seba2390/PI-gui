# Vendored crates

Third-party crates Pistachio builds with local changes, wired in through `[patch.crates-io]`
in the workspace `Cargo.toml`. Each keeps its upstream license file.

## gpui-pre-macos 0.3.14

Zed's `gpui_macos` (Apache-2.0), as published in the `gpui-pre` snapshot that gpui-kit
0.11.0 pins. Changed in `src/window.rs` only (search for `Pistachio:`):

- **Frames are paced by the `CVDisplayLink`, also on macOS 14+.** Upstream uses a
  `CAMetalDisplayLink` there. That link draws only inside its own callback, which macOS runs
  just before it delivers the same refresh's mouse events, so a hover always waited one full
  refresh; and it presents three refreshes after the callback.
- **Pointer input draws at once.** When a mouse move, press or scroll makes the window dirty
  (GPUI calls the frame waker), the frame is drawn in the event handler instead of at the
  next refresh. At most once per refresh; anything after that waits for the refresh.

Measured on a 75 Hz display, mouse move to the frame on screen (Metal's `presentedTime`):
63 ms before, 35 ms after (8–11 ms of that is macOS delivering the event).

- **A picture of the next frame on request** (`request_frame_capture`, `take_frame_capture`):
  the app blurs it behind its update prompt. GPUI renders a scene to an image only in test
  builds.

## gpui-pre-apple 0.3.14

Zed's `gpui_apple` (Apache-2.0), the Metal renderer `gpui-pre-macos` uses. Changed in
`src/metal_renderer.rs` only (search for `Pistachio:`): rendering a scene into an offscreen
texture and reading it back (`render_scene_to_image`) is compiled in normal builds, not only
with `test-support`/`bench-support`, for the frame capture above.

To update either: copy the new `gpui-pre-macos` release from `~/.cargo/registry/src/*/` over this
folder (without `Cargo.lock`, `Cargo.toml.orig` and `.cargo_vcs_info.json`), re-apply the
`Pistachio:` changes, and keep the version equal to the `gpui-pre` that gpui-kit pins (the same
for `gpui-pre-apple`). Drop the
patch once upstream GPUI paces frames this way.
