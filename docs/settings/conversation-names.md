# Conversation names

Give each new conversation a short name (about 3 to 6 words) made from its first message,
instead of the message's first line. Off by default.

The first message is sent to pi right away; its name arrives a moment later and is typed out in
the sidebar. The name is saved in the session (`set_session_name`), so the terminal pi shows it
too. Names you give a conversation yourself (Rename, or `/name`) are never changed. Very short
first messages are their own name, and slash commands are not named.

## Automatic names

```{image} /_static/screenshots/names-automatic-light.webp
:class: shot only-light
:alt: The Automatic names section
```

```{image} /_static/screenshots/names-automatic-dark.webp
:class: shot only-dark
:alt: The Automatic names section
```

Name new conversations
: Turns automatic names on.

Made by
: **The conversation's model**: one short extra request to the conversation's model, made by a
  separate `pi --print --no-session` with no tools, skills or extensions. The conversation, its
  history and the model's context stay exactly as in the terminal. For a session on an SSH
  host, it runs on the host.

  **A small model on this computer**: nothing is sent anywhere. The model runs on the CPU and
  is loaded only for the second or so a name takes. Until one is downloaded, new conversations
  keep their first line.

## Models on this computer

```{image} /_static/screenshots/names-models-light.webp
:class: shot only-light
:alt: The naming models
```

```{image} /_static/screenshots/names-models-dark.webp
:class: shot only-dark
:alt: The naming models
```

Used when names are made by a small model on this computer. Downloaded once from a pinned
Hugging Face revision and checked against its SHA-256; none ship with the app.

| Model | Download | Languages | |
|---|---|---|---|
| Qwen2.5 0.5B (Qwen, recommended) | 498 MB | many | about a second per name |
| SmolLM2 360M (Hugging Face) | 389 MB | English | smallest |
| Qwen2.5 1.5B (Qwen) | 1.1 GB | many | the best names, a few seconds each |

All three are Apache 2.0 licensed. Models are kept in `~/.pi/agent/pistachio/naming/`.
