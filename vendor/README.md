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

## gpui-base 0.11.0

gpui-kit's `gpui-base` (Apache-2.0), which parses and renders the chat's Markdown. Changed in
`src/text/format/markdown.rs` only (search for `Pistachio:`):

- **Formulas by pandoc's rules.** Upstream lets the `markdown` crate pair any two `$` as
  inline math, so in "**$4.0 trillion** — about **$4.02T**" the span between the dollars took
  the `**` on either side with it and the bold showed as asterisks. Now only `$` that delimit
  a formula by pandoc's rules reach the parser (`math_delimiters`): a formula opens with a `$`
  followed by a non-space and closes at the next `$`, which follows a non-space and is not
  followed by a digit, so "$5 and $10" stay prices. Other `$` outside code are swapped for a
  placeholder byte while parsing and turned back into `$` in the tree (`restore_dollars`).
  `\(…\)` and `\[…\]` (on lines of their own) are read as `$$…$$`. Every swap keeps the
  length, so source positions are unchanged.
- **Unclaimed math is prose.** When no plugin claims inline math (a text view without the
  app's math plugin, such as the update prompt), the whole text is parsed without it, since
  re-parsing only the span between the dollars leaves emphasis around it broken.
- One upstream test (`claimed_inline_math_survives_prose_flattening`) expects "$5 and $10"
  to be a formula; it now expects prices.

To update: copy the release gpui-kit pins from `~/.cargo/registry/src/*/gpui-base-*` over
`vendor/gpui-base` the same way and re-apply the change. Run its tests after a change: copy
`vendor/gpui-base` two folders deep into a scratch folder with the registry's `Cargo.lock` and
its `README.md` two levels above (a test includes `../../../../README.md`), then
`cargo test --lib --features test-support text::`.

To update the two GPUI crates: copy the new `gpui-pre-macos` release from `~/.cargo/registry/src/*/` over this
folder (without `Cargo.lock`, `Cargo.toml.orig` and `.cargo_vcs_info.json`), re-apply the
`Pistachio:` changes, and keep the version equal to the `gpui-pre` that gpui-kit pins (the same
for `gpui-pre-apple`). Drop the
patch once upstream GPUI paces frames this way.

## gpui-component 0.11.0

gpui-kit's `gpui-component` (Apache-2.0). Changed in `src/setting/` only (search for
`Pistachio:`):

- **Group titles are centred**, and the one picked in the Settings sidebar glows for a moment
  (`GroupFlash`; its colour is `Settings::flash_color`), to show where the page went. The glow
  follows the clock and asks for frames itself: GPUI's animations jump to their end when the
  system reduces motion, and this one only fades.
- **A thin line between groups** of a page.
- **A description runs up to its field.** Upstream stops a setting's text at three fifths of the
  row (`max_w_3_5`), so next to a switch it wrapped halfway across.

To update: copy the release gpui-kit pins from `~/.cargo/registry/src/*/gpui-component-*` over
`vendor/gpui-component` (without `Cargo.lock`, `Cargo.toml.orig` and `.cargo_vcs_info.json`) and
re-apply the changes.
