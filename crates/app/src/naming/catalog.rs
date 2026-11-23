//! The small models that can name conversations on this computer. None ship with the app:
//! each is downloaded on request from pinned Hugging Face revisions and checked against the
//! SHA-256 digests below.
//!
//! All are permissively licensed (Apache 2.0) instruction models in the official GGUF
//! quantizations of their makers, run by candle on the CPU (see [`titles::local`]). The
//! GGUF repositories hold no `tokenizer.json`, so it comes from the original model's.

use titles::local::Arch;

use crate::model_store::{Downloadable, Fetch, ModelFile, size_label};

/// A file and the pinned repository revision it comes from.
#[derive(Debug)]
pub struct Pinned {
    pub file: ModelFile,
    pub repo: &'static str,
    pub revision: &'static str,
}

impl Pinned {
    fn fetch(&self) -> Fetch {
        Fetch {
            name: self.file.name,
            size: self.file.size,
            sha256: self.file.sha256,
            url: format!(
                "https://huggingface.co/{}/resolve/{}/{}",
                self.repo, self.revision, self.file.name
            ),
        }
    }
}

#[derive(Debug)]
pub struct Model {
    /// Stable id: the settings value and the folder name under `naming/`.
    pub id: &'static str,
    pub name: &'static str,
    pub maker: &'static str,
    pub languages: &'static str,
    /// One line on what it is best for.
    pub note: &'static str,
    pub license: &'static str,
    pub arch: Arch,
    /// The quantized network (GGUF).
    pub weights: Pinned,
    pub tokenizer: Pinned,
}

impl Model {
    pub fn size_label(&self) -> String {
        size_label(self.size())
    }
}

impl Downloadable for Model {
    fn id(&self) -> &'static str {
        self.id
    }

    fn revision(&self) -> &'static str {
        self.weights.revision
    }

    fn fetches(&self) -> Vec<Fetch> {
        vec![self.weights.fetch(), self.tokenizer.fetch()]
    }
}

pub const DEFAULT_MODEL: &str = "qwen2.5-0.5b";

/// Qwen2.5's tokenizer: the same file for the 0.5B and 1.5B models.
const QWEN_TOKENIZER: ModelFile = ModelFile {
    name: "tokenizer.json",
    size: 7_031_645,
    sha256: "c0382117ea329cdf097041132f6d735924b697924d6f6fc3945713e96ce87539",
};

pub static MODELS: &[Model] = &[
    Model {
        id: "qwen2.5-0.5b",
        name: "Qwen2.5 0.5B",
        maker: "Qwen (Alibaba)",
        languages: "Many languages",
        note: "Recommended: good names in about a second.",
        license: "Apache 2.0",
        arch: Arch::Qwen2,
        weights: Pinned {
            file: ModelFile {
                name: "qwen2.5-0.5b-instruct-q4_k_m.gguf",
                size: 491_400_032,
                sha256: "74a4da8c9fdbcd15bd1f6d01d621410d31c6fc00986f5eb687824e7b93d7a9db",
            },
            repo: "Qwen/Qwen2.5-0.5B-Instruct-GGUF",
            revision: "9217f5db79a29953eb74d5343926648285ec7e67",
        },
        tokenizer: Pinned {
            file: QWEN_TOKENIZER,
            repo: "Qwen/Qwen2.5-0.5B-Instruct",
            revision: "7ae557604adf67be50417f59c2c2f167def9a775",
        },
    },
    Model {
        id: "smollm2-360m",
        name: "SmolLM2 360M",
        maker: "Hugging Face",
        languages: "English",
        note: "Smallest download; names English conversations only.",
        license: "Apache 2.0",
        arch: Arch::Llama,
        weights: Pinned {
            file: ModelFile {
                name: "smollm2-360m-instruct-q8_0.gguf",
                size: 386_404_992,
                sha256: "48ab3034d0dd401fbc721eb1df3217902fee7dab9078992d66431f09b7750201",
            },
            repo: "HuggingFaceTB/SmolLM2-360M-Instruct-GGUF",
            revision: "593b5a2e04c8f3e4ee880263f93e0bd2901ad47f",
        },
        tokenizer: Pinned {
            file: ModelFile {
                name: "tokenizer.json",
                size: 2_104_556,
                sha256: "9ca9acddb6525a194ec8ac7a87f24fbba7232a9a15ffa1af0c1224fcd888e47c",
            },
            repo: "HuggingFaceTB/SmolLM2-360M-Instruct",
            revision: "a10cc1512eabd3dde888204e902eca88bddb4951",
        },
    },
    Model {
        id: "qwen2.5-1.5b",
        name: "Qwen2.5 1.5B",
        maker: "Qwen (Alibaba)",
        languages: "Many languages",
        note: "The best names; a larger download and a few seconds per name.",
        license: "Apache 2.0",
        arch: Arch::Qwen2,
        weights: Pinned {
            file: ModelFile {
                name: "qwen2.5-1.5b-instruct-q4_k_m.gguf",
                size: 1_117_320_736,
                sha256: "6a1a2eb6d15622bf3c96857206351ba97e1af16c30d7a74ee38970e434e9407e",
            },
            repo: "Qwen/Qwen2.5-1.5B-Instruct-GGUF",
            revision: "91cad51170dc346986eccefdc2dd33a9da36ead9",
        },
        tokenizer: Pinned {
            file: QWEN_TOKENIZER,
            repo: "Qwen/Qwen2.5-1.5B-Instruct",
            revision: "989aa7980e4cf806f80c7fef2b1adb7bc71aa306",
        },
    },
];

pub fn model(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|model| model.id == id)
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_MODEL, MODELS, model};
    use crate::model_store::Downloadable as _;

    #[test]
    fn catalog_is_pinned_and_consistent() {
        assert!(model(DEFAULT_MODEL).is_some());
        for model in MODELS {
            crate::model_store::assert_pinned(model);
            let names: Vec<&str> = model.fetches().iter().map(|fetch| fetch.name).collect();
            assert_eq!(names.len(), 2);
            let weights = std::path::Path::new(names[0]);
            assert_eq!(weights.extension().and_then(|e| e.to_str()), Some("gguf"));
            assert_eq!(names[1], "tokenizer.json");
        }
        let mut ids: Vec<&str> = MODELS.iter().map(|model| model.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), MODELS.len(), "ids are unique");
    }

    #[test]
    fn urls_point_at_the_pinned_revisions() {
        let qwen = model(DEFAULT_MODEL).unwrap().fetches();
        assert_eq!(
            qwen[0].url,
            "https://huggingface.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF/resolve/\
             9217f5db79a29953eb74d5343926648285ec7e67/qwen2.5-0.5b-instruct-q4_k_m.gguf"
        );
        assert!(
            qwen[1]
                .url
                .contains("/Qwen/Qwen2.5-0.5B-Instruct/resolve/7ae5576")
        );
    }
}
