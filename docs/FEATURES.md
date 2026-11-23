# Features

Everything Pistachio does, in detail. For installation see the [README](../README.md).

- Chat in the centre: streaming markdown, collapsible thinking (“Thought for 4s”), tool calls with
  results and inline edit diffs, and “Worked for 1m 3s” after every turn.
- Bash calls read as what they do (“Read Cargo.toml, src/main.rs”, “Searched “fn main” in src”,
  “Ran cargo test”), worked out by the app from the command itself; the model writes nothing
  extra. Runs of calls that only look around (reading, listing, searching, checking) fold into
  one “Explored · N steps” row; expanding it shows every call. Anything not fully understood
  (loops, `$(…)`, `xargs`, unknown programs, flags that could change files such as
  `find -delete` or `sed -i`) is shown as the raw command, so a label never hides a change. A
  labelled call shows only its label; click it to see the command and its output. To check
  the labeller against real
  transcripts: `cargo run --release -p transcript --example label_corpus -- <session.jsonl…>`.
- Tools that run other tools (subagents, code mode) show that work inside their own row: a
  live step count while running, and every nested call and reply when expanded.
- Composer: attachments, working-folder picker, model and thinking pickers, context-window wheel,
  `/` command completion, inline `@file` completion (paths with spaces are quoted like pi does),
  ↑ / ↓ through earlier prompts, and `!cmd` shell commands. Messages typed while pi starts or is
  busy sending are listed above the input as “Waiting to send”, each with a cancel button.
- If pi stops (a crash or a failed start), the chat offers **Restart pi** on the same session;
  anything not yet sent goes back into the message box.
- Left sidebar with a **Chat | Code** switch (⌘1 / ⌘2):
  - **Code**: the coding agent in a project folder; sessions are grouped under their folder
    (collapsible, "+" for a new session there), including ones started from the terminal.
  - **Chat**: plain conversation with **no file or shell tools**: the model cannot read, write
    or run anything on your computer. Chats run pi in a dedicated empty folder
    (`~/.pi/agent/pistachio/chat`) with only deep search's two web tools (`--tools
    web_search,web_fetch --no-mcp`), which stay off until you turn web search on. Text
    attachments are pasted into the prompt instead of being referenced by path, and `!`
    shell commands are disabled.
  - **Web search** (Chat): the globe button next to the paperclip chooses how the model uses
    the web, per chat (remembered in the session; `/websearch off|search|deep` works too):
    - **Search**: a quick lookup when needed (a few searches and pages), then a concise answer
      with links.
    - **Deep search**: a thorough investigation: the model plans sub-questions, searches from
      several angles, reads many pages, cross-checks key facts and reports back with a
      structured, sourced report. Takes minutes rather than seconds.

    The model searches (`web_search`) and reads pages (`web_fetch`); the work is done in Rust
    by the app itself, and a small embedded pi extension only connects the tools to pi. Pages
    are read without JavaScript, reduced to their main text, and long pages are cut down to
    the passages that match what the model is looking for. Only public addresses are fetched
    (no localhost or private networks), and each message has a limit on searches and page
    reads. Settings live in `~/.pi/agent/pistachio/deepsearch/` and apply to the next message,
    no restart:
    - `config.json`: `backend` (`duckduckgo` by default, no key; `searxng` with
      `searxng.url`, e.g. a self-hosted instance with `json` enabled in its `settings.yml`;
      `brave` or `tavily` with an API key or `api_key_env`), `default_mode` for new chats,
      `modes.search` / `modes.deep` limits (`max_searches`, `max_fetches` per message),
      results per search, page text per read (`fetch.max_chars`; lower it for small context
      windows), `fetch.allow_private_networks` and timeouts. Settings added in later versions
      are filled in automatically.
    - `prompt-search.md` and `prompt-deep.md`: each mode's instructions (`{{date}}`,
      `{{max_searches}}` and `{{max_fetches}}` are filled in). Delete a file to get the
      default back.
    - Try settings without the GUI: `pistachio --deepsearch-tool search <query>` and
      `pistachio --deepsearch-tool fetch <url> [what to look for]` print what the model would
      see.
  - Search, rename and move to trash in both modes.
  - **Background work**: switching away from a chat that is working (or still has messages
    waiting to be sent) keeps it running. Its row shows a spinner; when it finishes it gets a
    green dot until you open it, a toast says so, and the Dock icon bounces if the app is not
    in front. Questions from extensions (confirm, select, input) in a background chat wait
    until you open that chat, marked with an amber dot, so they are never answered for the
    wrong chat; its toasts name the chat.
- **Branches**: hovering a prompt offers Copy and Edit; hovering the end of a turn offers
  Copy reply, Retry (newest turn) and Branch from here. Edit, Retry and branching continue in
  a new session (the original stays in the sidebar), like pi's `/fork`. Slash commands:
  `/fork` (pick a message), `/clone`, `/resume` (pick an earlier session here), `/export
  [path]` (HTML) and `/share` (a secret GitHub gist via `gh`, like pi's terminal).
- **SSH** (Code mode): the device button next to the folder ("This computer", with a status
  light) runs pi on another machine. "Add SSH connection…" lists the hosts in your
  `~/.ssh/config` (following `Include`), or takes a custom host, user, port, key and jump host;
  connections are saved in `~/.pi/agent/pistachio/hosts.json` and "Manage SSH connections…"
  renames them, changes the command that starts pi and removes them, as does each one's "⋮" in
  Settings → SSH connections, where a switch connects it or turns it off. pi runs **on the
  remote** (its tools, `AGENTS.md`, skills and sessions all live there)
  through the system `ssh`, so your config, keys, ssh-agent and `known_hosts` apply.
  - One shared connection per host (ControlMaster) makes later commands ~30 ms. With
    "Connect saved connections at start" (Settings → SSH connections, off by default) saved
    hosts connect quietly in the background when the app opens. The dot shows green (ready),
    amber (working or needs you), red (failed) or grey (not connected).
  - **pi is installed automatically** when a host does not have it, like an editor's remote
    server: into `~/.pistachio/server` on the host, with no root access, using the host's
    Node.js 22.19+ or a downloaded, checksum-verified standalone Node.js. Remove that folder
    to undo it.
  - **This computer's models work on every host**, with nothing to set up: the model list,
    default model and thinking level are the ones you have here, and model requests come back
    over the SSH connection and go out from this computer. So models on `localhost`, your LAN
    or a VPN work from any host, and keys and logins never leave this computer. A hidden pi
    here (started on first use) adds them to each request; the host reaches it through a
    port forward on its own loopback, and each request needs a token that only works for
    that host. pi on the host reads a settings folder Pistachio writes there,
    `~/.pistachio/agent` (written again when your models, logins or settings here change);
    the host's own `~/.pi` is not changed, and its extensions, skills, prompts and themes are
    linked in. Not yet relayed: ChatGPT/Codex, GitHub Copilot and Gemini CLI logins, Vertex
    and Bedrock. If a host does not allow port forwarding, pi there uses its own models and
    the model menu says why. "Add model" in a remote session edits this computer's models.
  - Passwords, key passphrases and one-time codes are asked for in the app (ssh's
    `SSH_ASKPASS`); nothing is stored. Unknown host keys are shown for confirmation; a changed
    host key is explained, and you can remove the old key and confirm the new one.
  - The folder browser takes a typed or pasted path, creates folders and can show hidden ones.
  - Remote sessions behave like local ones: they are listed in the sidebar under the host
    (then folder), can be opened, renamed and moved to the host's trash, and with "Reopen the
    last session" the last one comes back after a restart. The Diff and Files tabs read git and the folder over SSH
    (refreshed every 10 s while shown and after each turn), the terminal is your shell on the
    host in the session's folder, and the attach picker indexes the remote folder.
  - When the connection drops, the session offers to reconnect and resumes its session file.
  - Limits: csh/tcsh login shells are not supported (bash, zsh, dash, ksh and fish are; fish
    is untested on a real host). Windows clients are untested: Windows' OpenSSH cannot share
    connections, so every command opens its own connection there.
- **Skills** (Code mode): a Skills button under the Chat | Code switch shows the skills pi loaded
  for the session (personal ones from `~/.pi/agent/skills` and `~/.agents/skills`, project ones
  from `.pi/skills` and `.agents/skills`), as pi itself reports them, here or on the SSH host.
  "New skill" writes a `SKILL.md` (name checked against pi's rules, description, instructions,
  optionally "only when I call it" via `disable-model-invocation`) into your personal skills or
  the project's `.pi/skills`; existing skills open in an editor, with the toggle and move to
  trash. pi loads project resources only for trusted folders, and in RPC mode it cannot ask, so
  the page shows whether the project is trusted and can trust it, saved in pi's own
  `trust.json` like the terminal's `/trust`. pi reads skills when it starts, so changes restart
  it on the same session.
- **Formulas**: answers draw LaTeX math with KaTeX's layout and fonts: `$…$` and `\(…\)`
  inline, `$$…$$` and `\[…\]` as centered blocks (scrolling sideways when wider than the
  chat). Dollar signs follow pandoc's rules, so prices such as "$5 and $10" stay text, and
  code keeps its `$`. A formula that doesn't parse shows as written; copying gives its LaTeX.
- **Built-in skills**: Code sessions on this computer also load the skills that come with
  Pistachio, written to `~/.pi/agent/pistachio/skills` and passed with `--skill` (pi's own
  skill folders are not touched). They show read-only on the Skills page under "Comes with
  Pistachio". So far there is one, `add-skill`, which walks pi through adding a new skill.
- **Add model**: the model picker under the composer ends with "Add model…" and "Edit
  models.json…". The form adds a custom model to pi's `models.json`, to a provider already in
  the file or a new one (presets for Ollama, LM Studio, vLLM, llama.cpp and OpenRouter; any
  OpenAI-, Anthropic- or Gemini-compatible endpoint), and can list the server's models to pick
  from. The other tab edits the file directly, with validation. It is the file the session's pi
  reads (on the SSH host for remote sessions); the previous version is kept as
  `models.json.bak-pistachio`. pi reads the file only when it starts, so after saving it restarts
  on the same session (after the current turn, if one is running) and selects the new model.
- **Pictures**: when the agent reads an image, a thumbnail shows next to the call (the first two
  of an "Explored" group, with "+N"); pictures attached to a prompt show above it. Clicking one
  opens a full-window viewer: ← / → or the arrows browse all of that call's, group's or
  prompt's pictures, Esc closes it.
- **Working indicator**: a little green π with orbiting dots shows a working agent in the
  chat ("Working… 12s · esc to interrupt", centred) and before its name in the sidebar. Settings
  → Appearance → Working indicator picks the style, with a live preview.
- **Transcript layout**: the model's text is the main voice. Tool calls and thinking sit on a
  thin rail on the left in a quieter style, with more room where they meet the text.
- **Find in chat** (⌘F): a search bar over the conversation. Every match is highlighted;
  Enter / Shift+Enter (or ⌘G / ⌘⇧G) go to the next / previous one with a "3 of 12" count, and
  Esc closes the bar. It searches prompts and replies as shown, thinking, tool output and the
  agent's edits, including collapsed rows: the row with the current match opens and the
  transcript scrolls to it. Case is ignored.
- Right panel: git changes and a file tree of the working folder. Clicking a file opens it
  read-only in the panel, with syntax colors and line numbers (← goes back to the tree);
  **Mention** in its header, the **+** on a row, or ⌥-click in the tree puts an `@path`
  mention in the message box instead. Binary files show a short notice, and files over
  512 KB open with their first 512 KB. Files on an SSH host are read over the connection.
- **File links** (Code mode): a file of the session's folder that an answer names, in
  backticks (`fib.rs`) or as a path in the text (src/main.rs:12), is a link that opens it in
  the panel, at the line when one is given; the path of a Read, Edited or Wrote row does the
  same. A bare name is linked only when exactly one file in the folder has it. Links are
  added when an answer is finished (the folder is listed again when a turn ends, so files
  the turn made are found) and only in what the chat draws: pi, the session file and
  copying keep the answer as the model wrote it.
- Syntax colors for code by file extension: chat code blocks, the agent's edits (its diffs
  in the chat), the files it reads, and the Changes panel's diffs. Diffs keep their red and
  green backgrounds with the code colors on top; the old and new text are rebuilt from each
  diff and colored as a whole, so multi-line strings and comments come out right.
- Bottom panel: an integrated terminal in the working folder; drag its top edge to resize it
  (the height is remembered).
- macOS and Linux, plus an experimental Windows build. GPU-rendered with [GPUI](https://gpui.rs) via
  [gpui-kit](https://gpui-kit.com).

## Speech to text

The microphone button at the right of the message box dictates instead of typing, and so does
⌘⇧Space (Ctrl+Shift+Space on Linux/Windows) in the message box: hold it to talk and let go to
keep the text, or tap it to start and tap again to stop. Speech is
transcribed on this computer by a model downloaded once (none ship with the app); the first
press offers the choice:

| Model | Download | Languages | Speed |
|---|---|---|---|
| Parakeet v3 (NVIDIA, recommended) | 671 MB | 25 European languages, incl. English and Danish | very fast |
| Whisper large-v3 turbo (OpenAI) | 1.0 GB | about 99 languages | slower |
| Whisper small (OpenAI) | 375 MB | about 99 languages | fast |
| Moonshine v2 (Moonshine AI) | 141 MB | English only | fastest |

The download runs in the background with a progress bar at the top of the chat, from a pinned
Hugging Face revision, and every file is checked against its SHA-256 before the model is used.
While you speak, the text appears greyed out and keeps improving; when you stop talking (after
the silence set in Settings, 2.5 s by default), press Enter or press stop, it becomes normal
text in the message box, ready to edit or send. Esc discards it. Pauses and the end of speech
are found by a small voice detector built into the app (Silero VAD), which goes by how speech
sounds rather than how loud it is, so soft speech counts and a fan or typing does not. Models
live in `~/.pi/agent/pistachio/speech/`.

## Conversation names

Off by default (Settings → Conversation names). When on, a new conversation gets a short name
(about 3–6 words) made from its first message instead of the message's first line, and the
sidebar types it out letter by letter behind a caret. The message is sent to pi right away;
naming runs beside it and never holds the conversation up. The name is saved through pi
(`set_session_name`), so the terminal `pi` shows it too, and a name you gave the conversation
yourself (Rename, or `/name`) is never replaced. Very short first messages ("Fix the login
bug") are their own name, and slash commands are not named.

The name is made by either:

- **The conversation's own model**: one short request through a separate
  `pi --print --no-session` with the same provider and model (no tools, skills, context files
  or extensions, its own short instructions, thinking off). The conversation's pi, its
  session file and the model's context never see it, so the session stays exactly what the
  terminal shows. For a conversation on an SSH host, that pi runs on the host the same way.
- **A small model on this computer**, downloaded once (none ship with the app) from a pinned
  Hugging Face revision and checked against its SHA-256. It runs on the CPU (with
  [candle](https://github.com/huggingface/candle)) once per conversation, in the background,
  and is unloaded right after; nothing is sent anywhere.

| Model | Download | Languages | |
|---|---|---|---|
| Qwen2.5 0.5B (Qwen, recommended) | 498 MB | many | about a second per name |
| SmolLM2 360M (Hugging Face) | 389 MB | English | smallest |
| Qwen2.5 1.5B (Qwen) | 1.1 GB | many | the best names, a few seconds each |

All three are Apache 2.0 licensed. Models live in `~/.pi/agent/pistachio/naming/`.

## Settings and updates

Settings opens from the Pistachio menu (⌘,) or the button at the bottom of the sidebar. Every
change is saved right away.

- **General:** theme (follow the system, light or dark), code colors (the syntax palette for
  code in chat, diffs, skills and `models.json`, with a live preview), the working indicator's
  style (with a live preview), text size (scales the interface),
  whether to open in Chat, Code or the mode used last, reopening the last session, and
  notifications for chats working in the background.
- **pi & models:** which `pi` to run (`PISTACHIO_PI_BIN` still wins), the model and thinking
  level pi starts with (saved in pi's own `settings.json`, like `/settings` in the terminal),
  and shortcuts to add models or edit `models.json`.
- **Web search:** the search service (DuckDuckGo, SearXNG, Brave or Tavily, with API keys
  stored only on this computer), whether new chats search, limits per message, and local
  network access.
- **SSH connections:** whether saved connections connect when the app opens (off by
  default), and the saved hosts with their status, connecting, adding and managing them.
- **Speech to text:** the dictation models (download, use, remove), how long a silence ends
  dictation, and the language for Whisper models.
- **Conversation names:** automatic names for new conversations (off by default), whether the
  conversation's own model or a small model on this computer makes them, and the local models
  (download, use, remove).

The app's own settings live in `~/.pi/agent/pistachio/settings.json`.

**Updates.** When Pistachio opens (and from **Check for Updates…** in the Pistachio menu or
Settings) it asks GitHub for the latest release. If there is a newer one, a prompt in the middle
of the window shows what's new. **Update** installs it the way Pistachio was installed and opens
the new version: `brew upgrade --cask pistachio` for a Homebrew install on macOS (otherwise the
new app replaces the old one), the .deb or .rpm on Linux (the system asks for your password) or
the binary the tarball installed, and `pistachio.exe` on Windows. The download is checked against
GitHub's checksum; if anything fails, the old version stays and the prompt says what happened.
**Download instead** opens the download in your browser, and a version can be skipped. Nothing is
downloaded or installed without a yes, and the automatic check can be turned off in Settings.

**What's new.** After an update, Pistachio shows what changed since the version you had.

## Keyboard shortcuts

Composer behaviour mirrors pi's terminal.

| Key | Action |
|---|---|
| Enter / Shift+Enter | Send (steers while the agent works) / newline |
| Alt+Enter | Queue a follow-up message |
| Esc | Interrupt (queued messages return to the editor) |
| Alt+↑ | Restore queued messages to the editor |
| Shift+Tab | Cycle thinking level |
| Tab | Complete a `/` command or `@file` |
| ↑ / ↓ | Earlier / later prompts (in an empty message box) |
| ⌘⇧Space | Dictate: hold to talk, tap to start / stop |
| Ctrl+T / Ctrl+O | Expand all thinking / tool output |
| ⌘N / ⌘B / ⌘⌥B | New chat or session / toggle sidebar / toggle right panel |
| ⌘1 / ⌘2 | Chat mode / Code mode |
| ⌘F | Find in the conversation |
| Enter / Shift+Enter, ⌘G / ⌘⇧G | Next / previous match (in the find bar) |
| ⌘⇧D / ⌘⇧E | Changes / files panel |
| Ctrl+` | Toggle terminal |
| ⌘W / ⌘M | Close (the app keeps running) / minimize the window |
| ⌘, | Settings |

(Use Ctrl instead of ⌘ on Linux/Windows.)

In the terminal on macOS, Option-← / Option-→ move by word and ⌘-← / ⌘-→ / ⌘-Backspace go to
the start / end of the line or delete to its start, as in Terminal.app and iTerm. Each folder
keeps its own shell, so a dev server keeps running when you switch sessions.

Built-in pi commands that the TUI handles itself are mapped onto RPC: `/new`, `/compact`, `/name`,
`/model`, `/thinking`, `/session`, `/copy`, `/quit`. Extension commands, prompt templates and
`/skill:…` go to pi unchanged.
