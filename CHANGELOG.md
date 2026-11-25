# Changelog

All notable changes to PI-gui are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed

- The buttons that download models and document parsers have the download symbol.
- PDF and other files attached to a chat show as small pages above the message, like pictures,
  instead of their text in the message. Clicking a document opens it. Conversations are named after
  the message, not the text of the file.
- Pictures attached to a message say what kind of file they are in a corner, like the pages of files.
- What you attach shows in the message box as cards before it is sent: a small picture of the file
  with its extension on it, and its name. While a file is read, a spinner turns on its picture. They
  stay in the order they were attached in.

### Fixed

- The model is no longer told how an attached file was read, which it could take for a tool it does
  not have. pi's notes about pictures it made smaller no longer show in messages.
- The update prompt's progress bar fills as the new version downloads, also when Homebrew downloads
  it. Before, a Homebrew update showed a full, pulsing bar the whole time.

## [0.5.0] - 2026-11-25

### Added

- Deep search has an effort slider next to it in the message box: Light, Standard, Thorough or Max.
  More effort is more searches, pages and results per search, and a stricter check of the report.
  Each chat remembers its own; Settings → Web search has the one new chats start with. Effort is
  also a floor: the question is split into sub-questions, and a model that wants to report before
  it has searched and read enough for them, on enough different sites, is sent back to its research.
- Deep search checks its reports: whether the links work and were read, what the sources say about
  the claims (the key ones in Standard, all of them from Thorough on), and which claims, also in
  tables and bullets, have no source that was read. What it finds goes back to the model once or
  more. A symbol below the report opens how it did: what was searched and read, how many claims are
  backed, and what is still wrong, with the reasons and sources. A check that fails or is stopped
  says so.
- Only deep search's final report is written into the chat as the answer. Until then the chat shows
  the work: what was planned, searched, read and checked, and drafts as rows that open. If it is
  stopped, the latest complete draft is the answer, marked as not checked.
- Deep search reads PDF files.
- Settings → Document parsing chooses what reads PDF files. The built-in reader stays the default;
  PyMuPDF4LLM, Docling and MinerU can be downloaded for better tables, columns, formulas and scans.
  Each shows what it is good at, its size and its license before, is checked against checksums, runs
  without network, and can be removed. If it is missing, fails or takes too long, the built-in
  reader is used and the result says so.
- PDF files can be attached to chats. The chip says “Reading…” until it is done, and a long document
  is shortened to its outline and what matches the message. With Docling, Word, PowerPoint and Excel
  files can be attached too.
- pi reads PDF files as Markdown in Code sessions on this computer.
- PDF files that chats find on the web are read by the chosen parser.
- Long research no longer fills up the context: older pages are cut down to their key passages, and
  nothing is shown to the model twice. The “Working…” line says what it is doing.
- SearXNG can run on this computer, with one click and without Docker (macOS and Linux). Choosing
  it in Settings → Web search checks whether one answers at the address. If not, it shows what is
  missing and asks: Download and run gets SearXNG, a Python of its own and the packages it needs
  (about 80 MB) and starts it, Cancel keeps the service from before. A light shows whether it runs,
  and its “⋮” menu starts, stops, updates and removes it or changes the address. Only this computer
  can reach it. It starts when a chat searches and stops with Pistachio. It does not come with the
  app.

### Changed

- The thinking menu lists only the levels that differ on the model. Ones that its
  `thinkingLevelMap` in `models.json` sends as another level are left out: with minimal sent as
  low and high as xhigh, it has off, low, medium and xhigh. Shift+Tab steps through the same ones.
- Settings → Web search shows only what the chosen service needs: the SearXNG address, the Brave or
  the Tavily API key. The others keep what was entered. Searching the settings for one finds it.
- Settings → Web search has two sections that open with a click on their title: Advanced search
  options (the limits per message, and the text per page, now with sliders) and Advanced deep search
  settings (what each effort level may and has to do, and how its report is checked).

## [0.4.9] - 2026-11-24

### Changed

- The Skills page is an overview of all your skills, also without a session open: the ones that come
  with Pistachio, your own library, and each project's, beside each other, with "+" below them to
  add one. Skills are cards with an icon and color of their own; ones pi only loads when you call
  them say so. Opening or adding one expands beside the list.
- A new skill is for all projects or for one, chosen from a menu of your projects (also without a
  session open). Its instructions start with a template.
- An open skill has Move to trash, Discard and Save centered below it. Save is there when something
  was changed, and opening another skill no longer drops changes that are not saved.
- Text fields are as tall as the search field everywhere, and the focused one has a thin white
  outline (dark in the light theme) instead of a green glow.
- Switches in Settings have an outline and a light knob, and a brighter green when on. In the dark
  theme one that was off looked like it could not be clicked.
- The drifting π signs are behind every Settings page too.
- Settings → pi & models has Add model… and Edit models.json… centered below the text, as tall as
  the fields and with icons. Add connection in the SSH dialog is centered too, with an icon and
  the brighter green.
- A folder name too long for the message box's toolbar fades out instead of being cut off, then
  the machine's does. Pointing at the folder shows its path.

### Fixed

- A skill opened while another window was active came up empty, and Save would have emptied it.
- Clicking a file in a conversation while the Files tab is open shows the file. It closed the
  panel.
- The Skills page lists skills also when no session is open. It said "No skills yet".

## [0.4.8] - 2026-11-24

### Fixed

- Clicking a switch in Settings no longer leaves a green outline on it. The outline shows when the
  keyboard is used.
- Check for Updates shows one notification: the result takes the place of "Checking for updates…".
- macOS: the first connection to an SSH host on the local network goes through once you allow
  local network access. It failed while macOS asked, and had to be turned off and on again. If the
  host still can't be reached, the error says where the setting is, and macOS' question says what
  the access is for.

## [0.4.7] - 2026-11-24

### Added

- The `/` list works from the keyboard: ↑ / ↓ move through it, Tab or Enter takes the highlighted
  command and Esc closes it. Every match is listed (it scrolls), and matching looks inside names
  (`/add` finds `skill:add-skill`). Enter on a half-typed command completes it instead of sending
  it to the model.
- `/login` and `/logout` (optionally with a provider) open pi's sign-in in a terminal, `/reload`
  restarts pi on the conversation, and `/settings` opens Settings.

### Changed

- Settings → SSH connections: each saved connection has a switch that connects it and turns it
  off again, and a "⋮" menu with Rename, Change and Remove. Change edits what "Add SSH
  connection" asks for (host, user, port, key file, jump host) and the command that starts pi.
  A host from `~/.ssh/config` shows the config's values; what you change is saved in Pistachio
  and the file is left as it is. "+" below the list adds one. Turning a connection off ends everything running over it (it asks
  first when pi is working there), also stops one that is still connecting, and nothing in the
  background connects again until you turn it on or reconnect.
- The welcome screen's New session and New chat buttons, and New skill on the Skills page, are a
  quiet outline with a green icon instead of a solid green block; New chat shows its shortcut.
- macOS builds are for Apple Silicon only: the download is now `Pistachio-…-macos-arm64.zip`,
  half the size of the universal one. Intel Macs stay on 0.4.6, the last universal release:
  Homebrew won't install newer versions on them, and Update in 0.4.6 and earlier finds no
  universal download, so it says so and keeps the version they have. On Apple Silicon,
  Homebrew installs update as before; a copy installed from the zip updates to 0.4.7 once by
  hand (Update in 0.4.6 looks for the universal zip), and with Update from then on.

### Fixed

- API keys, cloud settings, proxies and other variables exported in your shell's startup files
  reach pi when Pistachio is started from the Dock or a desktop; only `PATH` and a few others did.
  A slow shell no longer freezes the window, and one that leaves a background process running no
  longer hangs it. If the shell can't be read, pi is still looked for in the usual folders.
- The SSH password prompt is ready to type in, and Enter sends the password; it cancelled instead.
  Enter in Rename, Add model, the SSH dialogs and an extension's text question does what the
  button does, instead of closing the dialog and losing what you typed. Extension choices, /resume
  and /fork work with the arrow keys and Enter.
- An extension's yes/no question is no longer answered "yes" by an Enter typed for the message box:
  it takes a click, or Enter on the button itself.
- A message you have not sent (and its attachments) is kept when you open another session, start
  a new one, switch between Chat and Code, restart pi, reconnect, or save a model or skill, until
  the app quits. A new session nothing was sent in comes back too, and choosing another folder or
  machine takes the message along.
- A message pi refuses (no API key, say) goes back to the message box with its images, followed by
  the messages waiting behind it. It was lost.
- `!commands` show their output as it comes, can be stopped with Esc or the stop button, and keep
  running when you switch chats.
- `/compact` shows that it is working, can be cancelled with Esc or the stop button, keeps going
  when you switch chats, and holds messages sent meanwhile.
- Esc and Alt+↑ give queued messages back with their images.
- `/model` takes an exact name over one that merely contains it and lists the candidates when
  several match; `/thinking` accepts only the levels the model offers.
- Renaming a session no longer opens it.
- One value in `settings.json` that can't be read no longer resets every setting: the others are
  kept, and so is a copy of the file.
- Reconnect starts pi again in an SSH session whose connection ended; it connected to the host
  but left the stopped session as it was.

## [0.4.6] - 2026-11-23

### Added

- A built-in skill, `add-skill`, that teaches pi how to add skills: where each kind goes, the
  front matter rules, writing a description pi can route on, supporting files, and checking
  that pi loads it. Ask pi to "make a skill for …" or call `/skill:add-skill`. Code sessions
  on this computer load it alongside your own skills (it is written to
  `~/.pi/agent/pistachio/skills` and passed with `--skill`; pi's own skill folders are left
  alone). The Skills page lists it under "Comes with Pistachio", read-only.
- Formulas in answers are drawn as math: `$…$` and `\(…\)` inline, `$$…$$` and `\[…\]`
  blocks centered on their own lines. KaTeX lays them out (a Rust port with KaTeX's fonts,
  so they look as they do on the web), sharp at any size and in the text color of either
  theme. A formula KaTeX can't read shows as written, and copying a formula gives its LaTeX.

### Fixed

- Naming a chat that starts with a question no longer answers it: Qwen2.5 0.5B named "what is
  current market cap of nvidia" "Nvidia's current market cap is $175 billion". The local
  model now sees a question among its examples (it names it "Nvidia current market cap"), the
  instructions say a question gets its topic as the title, and a title that states a number
  the message never mentions is rejected as an answer, so the chat keeps its first message.
- Dollar amounts no longer break bold text in answers: "**$4.0 trillion** — about **$4.02T**"
  rendered as `$4.0 trillion** — about **$4.02T`, because the text between two dollar signs
  was read as a formula. Dollar signs now follow pandoc's rules: a formula's closing `$` is
  never followed by a digit, so prices ("$5 and $10") stay text, and `$` in code is left
  alone.

## [0.4.5] - 2026-11-23

### Added

- Update from the app: when a new version is out, Update in the prompt installs it the way
  Pistachio was installed and opens the new version, with no download to unpack. On macOS
  that is `brew upgrade --cask pistachio` for a Homebrew install, otherwise the new app
  replaces the old one; on Linux the .deb or .rpm (the system asks for your password) or the
  binary the tarball installed; on Windows `pistachio.exe` is replaced. The download is
  checked against GitHub's checksum, and if anything fails the old version stays and the
  prompt says what happened. The prompt now sits in the middle of the window, over the window
  blurred (a dimmed veil on Linux and Windows).
- What's new: after an update, Pistachio shows what changed since the version you had, from
  this changelog.

### Fixed

- This computer's models on SSH hosts (#26):
  - The relay sends a request to the address in this computer's model settings, never to
    one taken from the request (only the path comes from it). The address was already
    checked against those settings; this also clears a code-scanning alert.
  - An answer that is not a stream (an error, a list of models) no longer carries this
    computer's key or provider headers back to the host, even from a server that writes the
    credentials it got into its answer.
  - When the SSH connection is made again in the moment before a session starts, pi on the
    host no longer starts without a way to reach this computer's models: the session stops
    with a message and offers to reconnect.
  - Relative paths in the host's own pi settings (`"extensions": ["./my-ext"]`, local
    packages) still point into the host's agent folder.

## [0.4.4] - 2026-11-23

### Added

- SSH sessions use this computer's models on any host, with no setup: the same model list,
  default model and thinking level. Model requests come back over the SSH connection and go
  out from this computer, so models on `localhost`, the LAN or a VPN work from any host, and
  keys and logins never leave this computer (each host gets a token that works only through
  its own connection). pi on the host uses a settings folder Pistachio manages,
  `~/.pistachio/agent`; the host's own `~/.pi` is left alone and its extensions and skills
  still load. ChatGPT/Codex, GitHub Copilot and Gemini CLI logins, Vertex and Bedrock are not
  relayed yet. The "Copy model settings" offer in the connect dialog and Manage SSH
  connections is gone.
- File names in answers are links (Code sessions): a file of the session's folder that an
  answer mentions (`fib.rs`, src/main.rs:12) opens in the Files tab, at its line, and so does
  the file of a Read, Edited or Wrote row. Only the chat's display changes: what pi and the
  model see, copying and the session file keep the answer as it was. A name is linked only
  when it points at exactly one file.

### Changed

- The default text size is larger (the old Large); the picker gains a Larger step and your
  current size is kept.
- Faster: the window appears sooner (about 235 ms instead of 450 ms in the benchmark; it no
  longer waits for the login shell's environment), and long conversations no longer lay out
  every message in one frame when opened or when the chat column changes width, so opening a
  conversation and toggling the sidebar or side panel take about half as long.
- A window that is not focused redraws at up to 60 fps instead of 30, so hovering it reacts
  sooner.

### Fixed

- The "Working…" label no longer shifts sideways while its shimmer passes over it.
- Hover highlights, clicks and scrolling show up sooner on macOS: about 35 ms from moving the
  mouse to the change on screen, down from about 63 ms on a 75 Hz display. GPUI waited for the
  next display refresh before drawing pointer input and then queued the frame three refreshes
  ahead; Pistachio now carries a patched copy of GPUI's macOS backend (see vendor/README.md).
- Deleting a session no longer freezes the window while Finder moves the file to the Trash:
  the row goes at once.
- Settings… and Check for Updates… in the app menu (and shortcuts like ⌘N) work right after
  launch instead of staying greyed out until something in the window was clicked.
- A turn's copy and branch buttons show while the pointer is anywhere in its answer, not only
  over the "Worked for" row.
- Hovering the open session in the sidebar no longer shows its rename and trash buttons on a
  darker patch: they take the hovered row's colour.
- Dictation keeps everything you said: the greyed-out live text no longer disappears when a
  re-decode comes back empty, and a phrase whose last decode is empty keeps the text that was
  shown instead of dropping it. Soft speech (a laptop's microphone at arm's length) is no
  longer cut into fragments, lost between phrases or ended by the silence timeout while you
  talk: pauses are found by a voice detector built into the app (Silero VAD) instead of by
  loudness.

## [0.4.3] - 2026-11-23

### Added

- Automatic conversation names (Settings → Conversation names, off by default): a new
  conversation is named after its first message in a few words, instead of the message's
  first line, and the sidebar types the name out behind a caret. The name is made either by
  the conversation's own model, through a separate `pi --print --no-session` call that leaves
  the conversation's session and context untouched (on the SSH host for remote
  conversations), or by a small model on this computer (Qwen2.5 0.5B, SmolLM2 360M or
  Qwen2.5 1.5B, downloaded on request, checked against pinned SHA-256 digests and run with
  candle). The first message is sent right away; names are saved through pi, so the
  terminal shows them too, and names you set yourself are never replaced.
- Pictures in the chat: when the agent reads an image (a screenshot, say), a small thumbnail
  shows to the right of the call, and an "Explored" group shows the first two with "+N".
  Pictures attached to a prompt show above it. Clicking one opens a full-window viewer; ← / →
  or the arrows browse all the pictures of that call, group or prompt, and Esc closes it.

### Changed

- Switching to a session is near-instant: its history is read straight from the session
  file (in about 60 ms for a long session) and shown while pi starts, instead of after pi
  has started and sent it (around a second). pi's own history replaces it only if it
  differs; sessions with compactions or context edits, and sessions on SSH hosts, still wait
  for pi.
- The working indicator is a little green π with dots orbiting it, in the chat and in the
  sidebar, where it now sits left of the conversation's name. Settings → Appearance →
  Working indicator picks the style, with a live preview: by default two dots chase round the
  π at a spinner's uneven speed while it glows and a shimmer crosses "Working…"; the others
  are a comet tail, a spinning π, a single dot, an atom ring, half-turn hops and a pulse. "Working… 12s · esc to interrupt" is centred, and "Worked for" has a
  small π.
- Tool calls start with a small icon for what they did (a document for reads, a pencil for
  changes, a terminal for commands, a magnifier for searches, a folder for listings, a globe
  for the web), instead of a green dot. It is muted when the call is done, red when it failed,
  breathing in the accent colour while it runs and faded when interrupted, so green is left
  for the connection and running work.
- The model's text stands apart from its activity: runs of tool calls and thinking sit on a
  thin rail on the left, in a slightly smaller, muted style, with more room where they meet
  the text. An image read says "Read an image" instead of "Read 1 line".

- Tool calls in the chat read in the past tense like the rest: "Wrote", "Edited", "Listed" and
  "Searched" instead of "Write", "Edit", "List" and "Search".

### Fixed

- A conversation name is never a code block's language ("rust"): when a small naming model
  answers with code instead of a title, the conversation keeps its first-line title.
- Opening a long conversation, the scrollbar now spans all of it from the start, instead of
  starting too big and shrinking and jumping as you scroll up.
- Clicking the conversation already behind the Settings or Skills page (or the Chat / Code
  mode already in use) did nothing; it now closes the page and shows the conversation.

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

[Unreleased]: https://github.com/seba2390/pistachio/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/seba2390/pistachio/compare/v0.4.9...v0.5.0
[0.4.9]: https://github.com/seba2390/pistachio/compare/v0.4.8...v0.4.9
[0.4.8]: https://github.com/seba2390/pistachio/compare/v0.4.7...v0.4.8
[0.4.7]: https://github.com/seba2390/pistachio/compare/v0.4.6...v0.4.7
[0.4.6]: https://github.com/seba2390/pistachio/compare/v0.4.5...v0.4.6
[0.4.5]: https://github.com/seba2390/pistachio/compare/v0.4.4...v0.4.5
[0.4.4]: https://github.com/seba2390/pistachio/compare/v0.4.3...v0.4.4
[0.4.3]: https://github.com/seba2390/pistachio/compare/v0.4.2...v0.4.3
[0.4.2]: https://github.com/seba2390/pistachio/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/seba2390/pistachio/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/seba2390/pistachio/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/seba2390/pistachio/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/seba2390/pistachio/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/seba2390/pistachio/releases/tag/v0.1.0
