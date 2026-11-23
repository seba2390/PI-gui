//! The speech-to-text models Pistachio offers. None ship with the app: each is downloaded
//! on request from a pinned Hugging Face revision and checked against the SHA-256 below.
//!
//! The files are sherpa-onnx's int8 exports. Sizes and speeds were measured on an Apple
//! Silicon Mac (4 CPU threads): Moonshine ~40×, Parakeet ~11×, Whisper small ~5× and
//! Whisper turbo ~2× faster than real time.

use crate::model_store::{Downloadable, Fetch};
pub use crate::model_store::{ModelFile, size_label};

/// How fast a model transcribes, relative to the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speed {
    Fastest,
    VeryFast,
    Fast,
    Slower,
}

impl Speed {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fastest => "Fastest",
            Self::VeryFast => "Very fast",
            Self::Fast => "Fast",
            Self::Slower => "Slower",
        }
    }
}

/// Which sherpa-onnx recognizer runs a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `NeMo` transducer: encoder, decoder, joiner.
    Parakeet,
    /// Moonshine v2: encoder and merged decoder.
    Moonshine,
    /// Whisper: `<prefix>-encoder`, `<prefix>-decoder`; takes a language hint.
    Whisper,
}

#[derive(Debug)]
pub struct Model {
    /// Stable id: the settings value and the folder name under `speech/`.
    pub id: &'static str,
    pub name: &'static str,
    pub maker: &'static str,
    pub languages: &'static str,
    pub speed: Speed,
    /// One line on what it is best for.
    pub note: &'static str,
    pub license: &'static str,
    pub kind: Kind,
    pub repo: &'static str,
    pub revision: &'static str,
    pub files: &'static [ModelFile],
}

impl Model {
    pub fn size_label(&self) -> String {
        size_label(self.size())
    }

    pub fn url(&self, file: &ModelFile) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.repo, self.revision, file.name
        )
    }

    /// The file named `suffix` exactly, or ending in `-suffix` (Whisper's `small-tokens.txt`).
    pub fn file(&self, suffix: &str) -> Option<&'static ModelFile> {
        self.files.iter().find(|file| {
            file.name == suffix
                || file
                    .name
                    .strip_suffix(suffix)
                    .is_some_and(|rest| rest.ends_with('-'))
        })
    }
}

impl Downloadable for Model {
    fn id(&self) -> &'static str {
        self.id
    }

    fn revision(&self) -> &'static str {
        self.revision
    }

    fn fetches(&self) -> Vec<Fetch> {
        self.files
            .iter()
            .map(|file| Fetch {
                name: file.name,
                size: file.size,
                sha256: file.sha256,
                url: self.url(file),
            })
            .collect()
    }
}

pub const DEFAULT_MODEL: &str = "parakeet-v3";

pub static MODELS: &[Model] = &[
    Model {
        id: "parakeet-v3",
        name: "Parakeet v3",
        maker: "NVIDIA",
        languages: "25 European languages, incl. English and Danish",
        speed: Speed::VeryFast,
        note: "Recommended: accurate, fast and adds punctuation.",
        license: "CC BY 4.0",
        kind: Kind::Parakeet,
        repo: "csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
        revision: "ed5d745623472f97cfc2e6c2916ee248798fc5a8",
        files: &[
            ModelFile {
                name: "encoder.int8.onnx",
                size: 652_282_294,
                sha256: "1afbe69c1ffdabee94755f42e4daf1d93549131a3da11356bda84fcef0f2c479",
            },
            ModelFile {
                name: "decoder.int8.onnx",
                size: 11_845_274,
                sha256: "0222a86147299d4dee90edf95b6d13827edb42ad8b7c9cf0b2582f030937c0b5",
            },
            ModelFile {
                name: "joiner.int8.onnx",
                size: 6_355_277,
                sha256: "cbd02f3696e9944970111fb439d9c3fd365a80374565f13f2b9c7007d253fc98",
            },
            ModelFile {
                name: "tokens.txt",
                size: 93_939,
                sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
            },
        ],
    },
    Model {
        id: "whisper-turbo",
        name: "Whisper large-v3 turbo",
        maker: "OpenAI",
        languages: "About 99 languages",
        speed: Speed::Slower,
        note: "Most accurate for less common languages; live text updates less often.",
        license: "MIT",
        kind: Kind::Whisper,
        repo: "csukuangfj/sherpa-onnx-whisper-turbo",
        revision: "2ca6ff69fc878651b770880507669577ac41c2ff",
        files: &[
            ModelFile {
                name: "turbo-encoder.int8.onnx",
                size: 674_716_297,
                sha256: "b02dcdf54f348741e93fe732b67d933c8dcb6735655f710640143081db38878b",
            },
            ModelFile {
                name: "turbo-decoder.int8.onnx",
                size: 361_080_764,
                sha256: "20accd02388482eb3a46bd615631adfdc85e1eb2c7db9ea3f02a40ffe6b81547",
            },
            ModelFile {
                name: "turbo-tokens.txt",
                size: 816_730,
                sha256: "b34b360dbb493e781e479794586d661700670d65564001f23024971d1f2fa126",
            },
        ],
    },
    Model {
        id: "whisper-small",
        name: "Whisper small",
        maker: "OpenAI",
        languages: "About 99 languages",
        speed: Speed::Fast,
        note: "Smaller download for many languages; less accurate.",
        license: "MIT",
        kind: Kind::Whisper,
        repo: "csukuangfj/sherpa-onnx-whisper-small",
        revision: "8f3c18b358db4d1f2fc1eae49d75cd20989e4309",
        files: &[
            ModelFile {
                name: "small-encoder.int8.onnx",
                size: 112_442_483,
                sha256: "4cbe7b22fa9026b843b60a68640c747de05bafb1a11b57edc0e66c232d9f33a9",
            },
            ModelFile {
                name: "small-decoder.int8.onnx",
                size: 262_226_114,
                sha256: "acad50b5c782696e91b55914cc5ab4f756f1532f76e22aa6fc615f39fb69a8ee",
            },
            ModelFile {
                name: "small-tokens.txt",
                size: 816_730,
                sha256: "b34b360dbb493e781e479794586d661700670d65564001f23024971d1f2fa126",
            },
        ],
    },
    Model {
        id: "moonshine-base-en",
        name: "Moonshine v2",
        maker: "Moonshine AI",
        languages: "English only",
        speed: Speed::Fastest,
        note: "Smallest and fastest; the smoothest live text in English.",
        license: "MIT",
        kind: Kind::Moonshine,
        repo: "csukuangfj2/sherpa-onnx-moonshine-base-en-quantized-2026-02-27",
        revision: "8f4d6c58c03d40bcea40043bb7120a878f2bbef6",
        files: &[
            ModelFile {
                name: "encoder_model.ort",
                size: 31_326_816,
                sha256: "7c66495948d0d08ec1af454cd4b5514862ae6511e94712a60e6d83eaec8dc8cf",
            },
            ModelFile {
                name: "decoder_model_merged.ort",
                size: 109_424_400,
                sha256: "d9d7b333af34bc552580576ddcf248a1c6c839e0d3b43b09afb9376ed009899d",
            },
            ModelFile {
                name: "tokens.txt",
                size: 549_350,
                sha256: "2870d843e14c1e187bf1913a521562a63b53933814bd7f2145120468f494a049",
            },
        ],
    },
];

pub fn model(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|model| model.id == id)
}

/// Languages offered for Whisper: (code, label). An empty code lets Whisper detect it.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("", "Detect automatically"),
    ("en", "English"),
    ("da", "Danish"),
    ("de", "German"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("it", "Italian"),
    ("nl", "Dutch"),
    ("no", "Norwegian"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("sv", "Swedish"),
    ("fi", "Finnish"),
    ("ja", "Japanese"),
    ("zh", "Chinese"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        assert!(model(DEFAULT_MODEL).is_some());
        for model in MODELS {
            crate::model_store::assert_pinned(model);
            let needed: &[&str] = match model.kind {
                Kind::Parakeet => &["encoder.int8.onnx", "decoder.int8.onnx", "joiner.int8.onnx"],
                Kind::Moonshine => &["encoder_model.ort", "decoder_model_merged.ort"],
                Kind::Whisper => &["encoder.int8.onnx", "decoder.int8.onnx"],
            };
            for name in needed.iter().chain(&["tokens.txt"]) {
                assert!(model.file(name).is_some(), "{}: no {name}", model.id);
            }
        }
    }
}
