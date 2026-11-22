//! The local recognizer: runs a downloaded model with sherpa-onnx on a worker thread.
//!
//! Audio arrives as 16 kHz mono. An energy-based voice detector splits it into phrases at
//! short pauses. While a phrase is being spoken, its audio is decoded again and again and
//! reported as a hypothesis (the composer shows it greyed out); at the pause it is decoded
//! once more and committed. After a longer silence the session ends by itself, as if the
//! user had pressed stop.
//!
//! The models are offline (whole-utterance) models, so "live" text is re-decoding: each
//! decode of a phrase costs a fraction of its length (see `catalog`), and the loop simply
//! decodes again as soon as the last decode finished and new speech arrived.

// Audio math converts between 16-bit samples, f32 samples and sample counts; all values are
// small and bounded (seconds of 16 kHz audio), so these casts cannot lose anything that matters.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::component::speech::{RecognitionSession, SpeechError, SpeechRecognizer, SpeechSink};
use gpui_kit::{App, AppContext as _};
use sherpa_onnx::{
    OfflineMoonshineModelConfig, OfflineRecognizer, OfflineRecognizerConfig,
    OfflineTransducerModelConfig, OfflineWhisperModelConfig,
};

use super::catalog::{Kind, Model};

pub const SAMPLE_RATE: usize = 16_000;
/// Voice detection works on 20 ms frames.
const FRAME: usize = SAMPLE_RATE / 50;
/// A pause this long after speech ends a phrase.
const PHRASE_PAUSE: usize = SAMPLE_RATE * 7 / 10;
/// Decode a hypothesis once at least this much new audio arrived.
const HYPOTHESIS_STEP: usize = SAMPLE_RATE * 2 / 5;
/// A phrase is committed at this length even without a pause, so decodes stay short.
const MAX_PHRASE: usize = SAMPLE_RATE * 25;
/// Audio kept before the first voiced frame of a phrase, so its first sound is not cut.
const LEAD_IN: usize = SAMPLE_RATE / 4;
/// With nothing said at all, give up after this long.
const NOTHING_SAID_STOP: usize = SAMPLE_RATE * 10;
/// A loaded model is freed after this long without a session.
const UNLOAD_AFTER: Duration = Duration::from_secs(10 * 60);

/// What a session needs to know, read from the settings when it starts.
#[derive(Clone)]
pub struct SessionConfig {
    pub model: &'static Model,
    pub dir: std::path::PathBuf,
    /// Whisper's language code; empty to detect it.
    pub language: String,
    /// End the session after this much silence; `None` waits for stop.
    pub auto_stop: Option<Duration>,
}

/// The loaded recognizer, shared by sessions and kept between them.
struct Loaded {
    key: (String, String),
    recognizer: Arc<OfflineRecognizer>,
}

static LOADED: Mutex<Option<Loaded>> = Mutex::new(None);
/// Bumped by every session start; the unload timer only fires if no session began since.
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn load(config: &SessionConfig) -> Result<Arc<OfflineRecognizer>, String> {
    let key = (config.model.id.to_owned(), config.language.clone());
    let mut loaded = LOADED.lock().map_err(|_| "recognizer lock poisoned")?;
    if let Some(loaded) = loaded.as_ref().filter(|loaded| loaded.key == key) {
        return Ok(loaded.recognizer.clone());
    }
    // Free the old model before loading the new one.
    *loaded = None;
    let recognizer = OfflineRecognizer::create(&recognizer_config(config))
        .ok_or_else(|| format!("could not load {}", config.model.name))?;
    let recognizer = Arc::new(recognizer);
    *loaded = Some(Loaded {
        key,
        recognizer: recognizer.clone(),
    });
    Ok(recognizer)
}

fn recognizer_config(config: &SessionConfig) -> OfflineRecognizerConfig {
    let model = config.model;
    let path = |suffix: &str| {
        model
            .file(suffix)
            .map(|file| config.dir.join(file.name).to_string_lossy().into_owned())
    };
    let mut c = OfflineRecognizerConfig::default();
    // More threads did not help on Apple Silicon (4 was fastest in our measurements).
    c.model_config.num_threads =
        std::thread::available_parallelism().map_or(2, |n| n.get().min(4)) as i32;
    c.model_config.tokens = path("tokens.txt");
    match model.kind {
        Kind::Parakeet => {
            c.model_config.transducer = OfflineTransducerModelConfig {
                encoder: path("encoder.int8.onnx"),
                decoder: path("decoder.int8.onnx"),
                joiner: path("joiner.int8.onnx"),
            };
            c.model_config.model_type = Some("nemo_transducer".into());
        }
        Kind::Moonshine => {
            c.model_config.moonshine = OfflineMoonshineModelConfig {
                encoder: path("encoder_model.ort"),
                merged_decoder: path("decoder_model_merged.ort"),
                ..Default::default()
            };
        }
        Kind::Whisper => {
            c.model_config.whisper = OfflineWhisperModelConfig {
                encoder: path("encoder.int8.onnx"),
                decoder: path("decoder.int8.onnx"),
                language: Some(config.language.clone()),
                task: Some("transcribe".into()),
                tail_paddings: -1,
                ..Default::default()
            };
        }
    }
    c
}

/// Frees the loaded model if no session started for [`UNLOAD_AFTER`].
fn schedule_unload(cx: &mut App) {
    let generation = GENERATION.load(Ordering::Relaxed);
    let timer = cx.background_executor().timer(UNLOAD_AFTER);
    cx.background_spawn(async move {
        timer.await;
        if GENERATION.load(Ordering::Relaxed) == generation
            && let Ok(mut loaded) = LOADED.lock()
        {
            *loaded = None;
        }
    })
    .detach();
}

/// Messages to the worker.
enum Input {
    Audio(Vec<i16>),
    Finish,
}

/// Messages from the worker.
enum Output {
    Ready,
    Hypothesis(String),
    Phrase(String),
    Finished,
    Failed(String),
}

/// The recognizer the composer's speech state uses. The session reads its model and
/// settings through `config`, called on the main thread when a session starts.
pub struct LocalRecognizer {
    pub config: ConfigSource,
}

/// Reads a session's [`SessionConfig`] from the app state; `None` while no model is installed.
pub type ConfigSource = Box<dyn Fn(&App) -> Option<SessionConfig>>;

impl SpeechRecognizer for LocalRecognizer {
    fn is_available(&self, cx: &App) -> bool {
        (self.config)(cx).is_some()
    }

    fn start(
        &self,
        sink: SpeechSink,
        cx: &mut App,
    ) -> Result<Box<dyn RecognitionSession>, SpeechError> {
        let config = (self.config)(cx).ok_or_else(|| {
            SpeechError::recognizer(anyhow::anyhow!("no speech model is installed"))
        })?;
        GENERATION.fetch_add(1, Ordering::Relaxed);
        let (input_tx, input_rx) = std::sync::mpsc::channel::<Input>();
        let (output_tx, output_rx) = async_channel::unbounded::<Output>();
        std::thread::Builder::new()
            .name("speech-recognizer".into())
            .spawn(move || run(config, input_rx, output_tx))
            .map_err(SpeechError::recognizer)?;
        let task = cx.spawn(async move |cx| {
            while let Ok(output) = output_rx.recv().await {
                let end = matches!(output, Output::Finished | Output::Failed(_));
                cx.update(|cx| match output {
                    Output::Ready => sink.ready(cx),
                    Output::Hypothesis(text) => sink.hypothesis(text, cx),
                    Output::Phrase(text) => sink.phrase(text, cx),
                    Output::Finished => sink.finish(cx),
                    Output::Failed(error) => {
                        sink.error(SpeechError::recognizer(anyhow::anyhow!(error)), cx);
                    }
                });
                if end {
                    break;
                }
            }
            cx.update(schedule_unload);
        });
        Ok(Box::new(Session {
            input: input_tx,
            _task: task,
        }))
    }
}

/// Dropping it closes the worker's input, which ends the worker without reporting more.
struct Session {
    input: std::sync::mpsc::Sender<Input>,
    _task: gpui_kit::Task<()>,
}

impl RecognitionSession for Session {
    fn push_audio(&mut self, samples: &[i16], _cx: &mut App) {
        let _ = self.input.send(Input::Audio(samples.to_vec()));
    }

    fn finish(&mut self, _cx: &mut App) {
        let _ = self.input.send(Input::Finish);
    }
}

/// The worker: loads the model, then turns audio into phrases until told to finish, the
/// silence timeout passes, or the session is dropped.
fn run(
    config: SessionConfig,
    input: std::sync::mpsc::Receiver<Input>,
    output: async_channel::Sender<Output>,
) {
    let send = |message| output.send_blocking(message).is_ok();
    let recognizer = match load(&config) {
        Ok(recognizer) => recognizer,
        Err(error) => {
            send(Output::Failed(error));
            return;
        }
    };
    if !send(Output::Ready) {
        return;
    }
    let mut phrases = Phrases::new(
        config
            .auto_stop
            .map(|d| (d.as_secs_f64() * SAMPLE_RATE as f64) as usize),
    );
    let decode = |samples: &[f32]| decode(&recognizer, &config, samples);
    loop {
        // Take all audio that arrived, then act once on it.
        let mut finish = false;
        match input.recv_timeout(Duration::from_millis(50)) {
            Ok(Input::Audio(samples)) => phrases.push(&samples),
            Ok(Input::Finish) => finish = true,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
        while let Ok(message) = input.try_recv() {
            match message {
                Input::Audio(samples) => phrases.push(&samples),
                Input::Finish => finish = true,
            }
        }

        let stop = finish || phrases.should_stop();
        if (stop || phrases.should_commit())
            && let Some(audio) = phrases.take_phrase()
        {
            let text = decode(&audio);
            if !text.is_empty() {
                let text = phrases.join(text);
                if !send(Output::Phrase(text)) {
                    return;
                }
            } else if !send(Output::Hypothesis(String::new())) {
                return;
            }
        } else if !stop && let Some(audio) = phrases.hypothesis_audio() {
            let text = decode(audio);
            let text = if text.is_empty() {
                text
            } else {
                phrases.preview(text)
            };
            if !send(Output::Hypothesis(text)) {
                return;
            }
        }
        if stop {
            send(Output::Finished);
            return;
        }
    }
}

fn decode(recognizer: &OfflineRecognizer, config: &SessionConfig, samples: &[f32]) -> String {
    let stream = recognizer.create_stream();
    if config.model.kind == Kind::Whisper {
        // Whisper stops at 6 tokens per second by default, which cuts off Danish.
        stream.set_option("max_tokens_per_second", "20");
    }
    stream.accept_waveform(SAMPLE_RATE as i32, samples);
    recognizer.decode(&stream);
    stream
        .get_result()
        .map(|result| clean(&result.text))
        .unwrap_or_default()
}

/// Trims and drops what models emit for silence or noise.
fn clean(text: &str) -> String {
    let text = text.trim();
    let bare = text.trim_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace());
    if bare.is_empty() {
        return String::new();
    }
    text.to_owned()
}

/// Splits incoming audio into phrases with a simple energy voice detector.
struct Phrases {
    /// Audio of the phrase being spoken (from shortly before its first voiced frame).
    current: Vec<f32>,
    /// Audio before speech started, trimmed to [`LEAD_IN`].
    lead_in: Vec<f32>,
    /// Samples not yet a whole frame.
    pending: Vec<f32>,
    /// The phrase has voiced frames.
    voiced: bool,
    /// Samples of silence since the last voiced frame.
    silence: usize,
    /// Samples of silence since the last voiced frame of any phrase (for auto-stop).
    idle: usize,
    /// Samples since the session began, while nothing was said.
    nothing_said: Option<usize>,
    /// `current.len()` at the last hypothesis decode.
    decoded_at: usize,
    noise_floor: f32,
    auto_stop: Option<usize>,
    committed_any: bool,
}

impl Phrases {
    fn new(auto_stop: Option<usize>) -> Self {
        Self {
            current: Vec::new(),
            lead_in: Vec::new(),
            pending: Vec::new(),
            voiced: false,
            silence: 0,
            idle: 0,
            nothing_said: Some(0),
            decoded_at: 0,
            noise_floor: 0.003,
            auto_stop,
            committed_any: false,
        }
    }

    fn push(&mut self, samples: &[i16]) {
        self.pending
            .extend(samples.iter().map(|&s| f32::from(s) / 32_768.));
        let whole = self.pending.len() / FRAME * FRAME;
        let frames: Vec<f32> = self.pending.drain(..whole).collect();
        for frame in frames.chunks(FRAME) {
            self.push_frame(frame);
        }
    }

    fn push_frame(&mut self, frame: &[f32]) {
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt();
        let voiced = rms > (self.noise_floor * 3.).max(0.008);
        if !voiced {
            // Follow the room's noise slowly, and quickly when it gets quieter.
            let rate = if rms < self.noise_floor { 0.2 } else { 0.02 };
            self.noise_floor += (rms - self.noise_floor) * rate;
        }
        if voiced {
            if !self.voiced {
                self.current = std::mem::take(&mut self.lead_in);
                self.decoded_at = 0;
            }
            self.voiced = true;
            self.silence = 0;
            self.idle = 0;
            self.nothing_said = None;
            self.current.extend_from_slice(frame);
        } else if self.voiced {
            self.silence += frame.len();
            self.idle += frame.len();
            self.current.extend_from_slice(frame);
        } else {
            self.idle += frame.len();
            if let Some(n) = self.nothing_said.as_mut() {
                *n += frame.len();
            }
            self.lead_in.extend_from_slice(frame);
            if self.lead_in.len() > LEAD_IN {
                let extra = self.lead_in.len() - LEAD_IN;
                self.lead_in.drain(..extra);
            }
        }
    }

    /// A pause ended the phrase, or it grew too long.
    fn should_commit(&self) -> bool {
        self.voiced && (self.silence >= PHRASE_PAUSE || self.current.len() >= MAX_PHRASE)
    }

    /// The silence timeout passed (or nothing was said for a long while).
    fn should_stop(&self) -> bool {
        let silent =
            self.auto_stop.is_some_and(|limit| self.idle >= limit) && self.nothing_said.is_none();
        let never_spoke = self.nothing_said.is_some_and(|n| n >= NOTHING_SAID_STOP);
        silent || never_spoke
    }

    /// The finished phrase's audio, if it had speech.
    fn take_phrase(&mut self) -> Option<Vec<f32>> {
        if !self.voiced {
            return None;
        }
        self.voiced = false;
        self.silence = 0;
        self.decoded_at = 0;
        Some(std::mem::take(&mut self.current))
    }

    /// The current phrase's audio when enough is new to decode again.
    fn hypothesis_audio(&mut self) -> Option<&[f32]> {
        if !self.voiced || self.current.len() < self.decoded_at + HYPOTHESIS_STEP {
            return None;
        }
        self.decoded_at = self.current.len();
        Some(&self.current)
    }

    /// A committed phrase, with the space that separates it from the one before.
    fn join(&mut self, text: String) -> String {
        let text = self.preview(text);
        self.committed_any = true;
        text
    }

    /// Text shown after the committed phrases, with its separating space.
    fn preview(&self, text: String) -> String {
        if self.committed_any {
            format!(" {text}")
        } else {
            text
        }
    }
}

/// Audio from a 16 kHz mono WAV file at real-time speed, then silence: replaces the
/// microphone when `PISTACHIO_SPEECH_WAV` is set, for scripted checks of dictation.
pub struct WavInput {
    pub samples: Arc<Vec<i16>>,
}

impl WavInput {
    pub fn from_env() -> Option<Self> {
        let path = std::env::var("PISTACHIO_SPEECH_WAV").ok()?;
        let wave = sherpa_onnx::Wave::read(&path)?;
        let samples = wave
            .samples()
            .iter()
            .map(|s| (s * 32_767.) as i16)
            .chain(std::iter::repeat_n(0, SAMPLE_RATE * 6))
            .collect();
        Some(Self {
            samples: Arc::new(samples),
        })
    }
}

impl gpui_kit::component::speech::AudioInput for WavInput {
    fn start(
        &self,
        _format: gpui_kit::component::speech::AudioFormat,
        sink: gpui_kit::component::speech::AudioSink,
        cx: &mut App,
    ) -> Result<gpui_kit::Subscription, SpeechError> {
        let samples = self.samples.clone();
        let task = cx.spawn(async move |cx| {
            for chunk in samples.chunks(FRAME) {
                cx.background_executor()
                    .timer(Duration::from_millis(20))
                    .await;
                cx.update(|cx| sink.push(chunk.to_vec(), cx));
            }
        });
        Ok(gpui_kit::Subscription::new(move || drop(task)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(seconds: f32, amplitude: f32) -> Vec<i16> {
        let n = (seconds * SAMPLE_RATE as f32) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / SAMPLE_RATE as f32;
                ((t * 440. * std::f32::consts::TAU).sin() * amplitude * 32_767.) as i16
            })
            .collect()
    }

    #[test]
    fn a_pause_commits_a_phrase() {
        let mut phrases = Phrases::new(Some(SAMPLE_RATE * 3));
        phrases.push(&tone(0.5, 0.0005));
        assert!(!phrases.voiced, "room noise is not speech");
        phrases.push(&tone(1.0, 0.3));
        assert!(phrases.voiced);
        assert!(phrases.hypothesis_audio().is_some());
        assert!(!phrases.should_commit());
        phrases.push(&tone(0.8, 0.0005));
        assert!(phrases.should_commit(), "0.8 s of silence ends the phrase");
        let audio = phrases.take_phrase().unwrap();
        // The lead-in plus the speech plus the pause.
        assert!(audio.len() > SAMPLE_RATE * 18 / 10, "{}", audio.len());
        assert!(!phrases.should_stop());
        phrases.push(&tone(2.5, 0.0005));
        assert!(phrases.should_stop(), "3 s after the last speech");
    }

    #[test]
    fn stops_when_nothing_is_said() {
        let mut phrases = Phrases::new(None);
        phrases.push(&tone(9.0, 0.0005));
        assert!(!phrases.should_stop());
        phrases.push(&tone(1.5, 0.0005));
        assert!(phrases.should_stop());
    }

    #[test]
    fn phrases_are_joined_with_spaces() {
        let mut phrases = Phrases::new(None);
        assert_eq!(phrases.join("Hello there.".into()), "Hello there.");
        assert_eq!(phrases.preview("How are".into()), " How are");
        assert_eq!(phrases.join("How are you?".into()), " How are you?");
    }

    /// End to end with a real model: `PISTACHIO_STT_TEST=<model dir>:<model id>:<16 kHz wav>
    /// cargo test -p pistachio real_model -- --ignored --nocapture`. Streams the clip in 20 ms
    /// chunks as the microphone would, then silence, and prints what the worker reports.
    #[test]
    #[ignore = "needs a downloaded model"]
    fn real_model() {
        let spec = std::env::var("PISTACHIO_STT_TEST").expect("PISTACHIO_STT_TEST");
        let mut parts = spec.splitn(3, ':');
        let (dir, id, wav) = (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        );
        let wave = sherpa_onnx::Wave::read(wav).expect("wav");
        let samples: Vec<i16> = wave
            .samples()
            .iter()
            .map(|s| (s * 32_767.) as i16)
            .chain(std::iter::repeat_n(0, SAMPLE_RATE * 4))
            .collect();
        let config = SessionConfig {
            model: crate::stt::catalog::model(id).unwrap(),
            dir: dir.into(),
            language: String::new(),
            auto_stop: Some(Duration::from_millis(2500)),
        };
        let (input_tx, input_rx) = std::sync::mpsc::channel();
        let (output_tx, output_rx) = async_channel::unbounded();
        let worker = std::thread::spawn(move || run(config, input_rx, output_tx));
        let started = std::time::Instant::now();
        for chunk in samples.chunks(FRAME) {
            if input_tx.send(Input::Audio(chunk.to_vec())).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(input_tx);
        worker.join().unwrap();
        let mut phrases = String::new();
        let mut finished = false;
        while let Ok(output) = output_rx.try_recv() {
            match output {
                Output::Ready => println!("[{:?}] ready", started.elapsed()),
                Output::Hypothesis(text) => println!("  … {text}"),
                Output::Phrase(text) => {
                    println!("  = {text}");
                    phrases.push_str(&text);
                }
                Output::Finished => finished = true,
                Output::Failed(error) => panic!("{error}"),
            }
        }
        println!("final: {phrases}");
        assert!(finished, "silence ends the session");
        assert_ne!(phrases, "", "something was transcribed");
    }

    #[test]
    fn cleans_noise_only_results() {
        assert_eq!(clean("  . "), "");
        assert_eq!(clean(" Hej. "), "Hej.");
    }
}
