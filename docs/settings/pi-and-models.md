# pi & models

Which pi Pistachio runs, the model new sessions start with, and the models you have connected.

## pi

```{image} /_static/screenshots/pi-pi-light.webp
:class: shot only-light
:alt: The pi section
```

```{image} /_static/screenshots/pi-pi-dark.webp
:class: shot only-dark
:alt: The pi section
```

pi program
: The `pi` to run. Leave it empty to find pi on your `PATH`. The page shows the version it
  found. Used by new sessions. If `PISTACHIO_PI_BIN` is set, it takes precedence.

## Startup model

```{image} /_static/screenshots/pi-startup-light.webp
:class: shot only-light
:alt: The Startup model section
```

```{image} /_static/screenshots/pi-startup-dark.webp
:class: shot only-dark
:alt: The Startup model section
```

The model and thinking level new sessions start with, or **Let pi choose**. They are saved in
pi's own `settings.json`, like `/settings` in the terminal, so the terminal pi uses them too.

## Connected models

```{image} /_static/screenshots/pi-connected-light.webp
:class: shot only-light
:alt: Connected models
```

```{image} /_static/screenshots/pi-connected-dark.webp
:class: shot only-dark
:alt: Connected models
```

The servers and API keys in `models.json` on this computer, grouped by where the models run:
**This device**, **Other devices** and **API connections**. Each shows whether its server
answers; click one to change it. The trash button removes it after asking: an API connection or
a server on another device is only removed from `models.json`, and so is a server on this device
that Pistachio did not set up (like Ollama). A model Pistachio downloaded and runs on this device
is also stopped and deleted, with its engine unless another local model uses it. Such a model
also has a play / stop button: play starts its engine and loads the model (the row says
**Starting…** until it answers), stop ends it; its files stay. Sign-ins made
with `/login` are not listed here.

**Add models…** (also **Add or change models…** in the model menu of the message box) starts
with two choices:

Locally hosted model
: Choose whether it runs on this device or another one (by its address, like
  `192.168.1.89`). On this device (when pi runs here) there are two ways:
  **Attach to existing model** and **Deploy new model**. Attaching (and another device) looks
  for Ollama, LM Studio, llama.cpp, MLX, vLLM, Jan, Docker Model Runner, SGLang, Lemonade and
  KoboldCpp on their usual ports. Any other server is connected by its port or address;
  Pistachio finds out whether it speaks OpenAI's, Anthropic's or Gemini's API.

  Each choice opens a page of its own, and the back arrow returns to the one before.

  **Deploy new model** suggests models for this computer. **Scan local hardware** looks at the
  processor, the memory, the graphics, the free disk and the build tools, with a progress bar
  that says what it is looking at, and lists what it found; nothing leaves the computer.
  **Suggest models** then offers up to three: **Fast**, **Balanced** and **Most capable**, each faster and less capable than the next. Each starts collapsed;
  the arrow at its bottom opens it. Opened, it shows the
  inference engine that runs it best here, the checkpoint and its quantization, the memory it
  takes and its speed here (measured or estimated); the engine and the checkpoint link to their
  pages. Each comes with a
  default engine that installs without any steps (llama.cpp, Rapid-MLX on Macs, Splash on newer
  Macs, BigMoeOnEdge for models larger than the memory) and, where one is clearly faster, a
  specialized engine that is built on this computer (ik_llama.cpp, TurboFieldfare). The **Download and
  run** button says how much it is; pressing it shows exactly what is downloaded (the engine and
  the model's files, linked to their pages, with their sizes and licenses) and asks first. Everything is pinned to a version and
  checked against its SHA-256, and kept in `~/.pi/agent/pistachio/local-models`. Once it runs,
  the model is connected like any other server. It runs for this computer only, is started when
  a chat sends to it, and stops when Pistachio quits.

API provider model
: pi's own providers (Anthropic, OpenAI, Google Gemini, OpenRouter and more). Paste an API key
  (or `$NAME` for an environment variable, or `!command`), which is checked with the provider
  where it allows that, or sign in. All the provider's models then appear in the model menu.

For a server, Pistachio asks which models it has and you tick the ones to add. Their context
size, longest answer, and whether they think or see pictures are filled in as far as the server
says, and can be changed. A model can be tested with a short message before it is added, and
added by its ID when the server has no list.

**Edit models.json…** opens the file as text, with validation. The previous version is kept as
`models.json.bak-pistachio`.

pi reads `models.json` only when it starts, so after a change it restarts on the same session
(after the current turn) and selects the new model. In a session on an SSH host, the models are
still the ones on this computer (see [SSH connections](ssh-connections.md#models-on-ssh-hosts)).
