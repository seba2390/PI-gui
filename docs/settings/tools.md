# Tools

What the model can use in each mode, and how much of its context that takes: every tool's
description goes along with each message. Changes apply to conversations started afterwards.

The page asks pi itself which tools a conversation of each mode gets, so your extensions' tools
are listed too.

## Chat

```{image} /_static/screenshots/tools-chat-light.webp
:class: shot only-light
:alt: The tools of chats
```

```{image} /_static/screenshots/tools-chat-dark.webp
:class: shot only-dark
:alt: The tools of chats
```

No file or shell tools: chats can't touch your computer.

| Tool | What it does | Offered |
|---|---|---|
| `python` | Runs short Python programs in a sealed sandbox: maths, text, dates, checks | When its switch is on (the default) |
| `web_search` | Searches the web: titles, links and short snippets | While web search is on in the chat |
| `web_fetch` | Reads a web page or PDF as text | While web search is on in the chat |

The Python switch is the same as **Compute with Python** under
[Settings → General](../general.md#settings-general).

## Code

```{image} /_static/screenshots/tools-code-light.webp
:class: shot only-light
:alt: The tools of Code sessions
```

```{image} /_static/screenshots/tools-code-dark.webp
:class: shot only-dark
:alt: The tools of Code sessions
```

pi's tools and your extensions', in sessions on this computer.

| Tool | What it does |
|---|---|
| `read` | Reads a file, or looks at an image, in the project |
| `bash` | Runs a shell command in the project folder |
| `edit` | Changes part of a file by replacing exact text |
| `write` | Creates a file, or replaces all of its content |
| `grep` | Searches the contents of files (optional) |
| `find` | Finds files by name pattern (optional) |
| `ls` | Lists a folder's contents (optional) |
| `codemode` | Writes a short script that calls several tools at once and keeps only what matters (optional) |

The optional tools are off until you turn them on with their switch. They are passed to pi as
`--tools +grep,…`, so the terminal pi isn't affected. The
[sandbox](sandboxing-and-security.md) checks `grep`, `find` and `ls` like `read`, and
`codemode`'s scripts reach files and commands only through the session's tools, which the same
checks cover.

## The context meter

```{image} /_static/screenshots/tools-meter-light.webp
:class: shot only-light
:alt: The context meter
```

```{image} /_static/screenshots/tools-meter-dark.webp
:class: shot only-dark
:alt: The context meter
```

Each mode starts with about how many tokens its tools take with every message, and a bar with
each tool's share. It is counted from the text pi sends for each tool (description, parameters
and guidelines in the system prompt), at about 4 characters a token. In chats it also shows the
total with web search on.

## What the model sees

```{image} /_static/screenshots/tools-model-sees-light.webp
:class: shot only-light
:alt: A tool with its What the model sees link
```

```{image} /_static/screenshots/tools-model-sees-dark.webp
:class: shot only-dark
:alt: A tool with its What the model sees link
```

Each tool has a one-line summary, where it comes from (pi, Pistachio or an extension) and its
size. **What the model sees** opens, word for word, what pi tells the model: the description,
the guidelines in the system prompt and the parameters.

The text of Pistachio's own tools (`python`, `web_search`, `web_fetch` and the sandboxed `bash`)
can be changed:

- **Edit** turns the description and guidelines into fields. They are saved as you type to
  `~/.pi/agent/pistachio/tools/texts.json` and used by conversations started afterwards.
- **Use the original text** undoes your changes.
