# Speech to text

Dictate messages instead of typing them. Speech is transcribed by a model that runs on this
computer; nothing is sent anywhere.

## Dictating

Click the microphone button at the right of the message box, or use ⌘⇧Space (Ctrl+Shift+Space
on Linux and Windows) in the message box: hold it to talk and let go to keep the text, or tap it
to start and tap again to stop.

While you speak, the text appears greyed out and keeps improving. When you stop talking (after
the silence set below), press Enter or press stop, it becomes normal text in the message box,
ready to edit or send. Esc discards it.

Pauses are found by a small voice detector built into the app (Silero VAD). It goes by how
speech sounds rather than how loud it is, so soft speech counts and a fan or typing does not.

## Models

```{image} /_static/screenshots/speech-models-light.webp
:class: shot only-light
:alt: The speech models
```

```{image} /_static/screenshots/speech-models-dark.webp
:class: shot only-dark
:alt: The speech models
```

No model ships with the app. The first press of the microphone offers the choice, and this page
lists them to download, use or remove:

| Model | Download | Languages | Speed |
|---|---|---|---|
| Parakeet v3 (NVIDIA, recommended) | 671 MB | 25 European languages, incl. English and Danish | very fast |
| Whisper large-v3 turbo (OpenAI) | 1.0 GB | about 99 languages | slower |
| Whisper small (OpenAI) | 375 MB | about 99 languages | fast |
| Moonshine v2 (Moonshine AI) | 141 MB | English only | fastest |

A download asks first and shows its files and their sizes. It runs in the background with a
progress bar, from a pinned Hugging Face revision, and every file is checked against its SHA-256
before the model is used. Models are kept in `~/.pi/agent/pistachio/speech/`.

## Dictation

```{image} /_static/screenshots/speech-dictation-light.webp
:class: shot only-light
:alt: The Dictation section
```

```{image} /_static/screenshots/speech-dictation-dark.webp
:class: shot only-dark
:alt: The Dictation section
```

Stop listening
: Dictation ends by itself after this much silence: 1.5, 2.5 (the default) or 4 seconds, or
  only when you press stop. Enter or the stop button end it at any time; Esc discards it.

Language (Whisper)
: Whisper is more accurate when told the language. Parakeet detects it by itself; Moonshine
  understands English only.
