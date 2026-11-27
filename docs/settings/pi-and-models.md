# pi & models

Which pi Pistachio runs, the model new sessions start with, and the models you have connected.

## pi

pi program
: The `pi` to run. Leave it empty to find pi on your `PATH`. The page shows the version it
  found. Used by new sessions. If `PISTACHIO_PI_BIN` is set, it takes precedence.

## Startup model

The model and thinking level new sessions start with, or **Let pi choose**. They are saved in
pi's own `settings.json`, like `/settings` in the terminal, so the terminal pi uses them too.

## Connected models

The servers and API keys in `models.json` on this computer, each with whether its server
answers. Click one to change or remove it. Sign-ins made with `/login` are not listed here.

**Add models…** (also **Add or change models…** in the model menu of the message box) starts
with two choices:

Locally hosted model
: Choose whether it runs on this device or another one (by its address, like
  `192.168.1.89`). Pistachio looks for Ollama, LM Studio, llama.cpp, MLX, vLLM, Jan, Docker
  Model Runner, SGLang, Lemonade and KoboldCpp on their usual ports. Any other server is
  connected by its port or address; Pistachio finds out whether it speaks OpenAI's,
  Anthropic's or Gemini's API.

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
