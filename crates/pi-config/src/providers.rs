//! The providers pi knows itself: with a key (or a sign-in), all their models are there.

/// How to ask a provider whether it takes a key, without using any tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyCheck {
    pub url: &'static str,
    /// The API whose headers carry the key (see [`crate::server::headers`]).
    pub api: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltIn {
    pub id: &'static str,
    pub label: &'static str,
    /// The environment variable pi reads the key from.
    pub env: &'static str,
    /// Where to get a key.
    pub keys_url: Option<&'static str>,
    pub check: Option<KeyCheck>,
    /// What one can sign in to it with (`/login`), instead of a key.
    pub sign_in: Option<&'static str>,
}

const fn provider(id: &'static str, label: &'static str, env: &'static str) -> BuiltIn {
    BuiltIn {
        id,
        label,
        env,
        keys_url: None,
        check: None,
        sign_in: None,
    }
}

impl BuiltIn {
    const fn keys(mut self, url: &'static str) -> Self {
        self.keys_url = Some(url);
        self
    }

    const fn check(mut self, url: &'static str) -> Self {
        self.check = Some(KeyCheck {
            url,
            api: "openai-completions",
        });
        self
    }

    const fn check_as(mut self, api: &'static str, url: &'static str) -> Self {
        self.check = Some(KeyCheck { url, api });
        self
    }

    const fn sign_in(mut self, with: &'static str) -> Self {
        self.sign_in = Some(with);
        self
    }
}

/// How many of [`BUILT_IN`] are the best known: they come first.
pub const BEST_KNOWN: usize = 4;

/// The providers that need no more than a key.
pub const BUILT_IN: &[BuiltIn] = &[
    provider("anthropic", "Anthropic", "ANTHROPIC_API_KEY")
        .keys("https://console.anthropic.com/settings/keys")
        .check_as("anthropic-messages", "https://api.anthropic.com/v1/models")
        .sign_in("Claude Pro or Max"),
    provider("openai", "OpenAI", "OPENAI_API_KEY")
        .keys("https://platform.openai.com/api-keys")
        .check("https://api.openai.com/v1/models")
        .sign_in("ChatGPT"),
    provider("google", "Google Gemini", "GEMINI_API_KEY")
        .keys("https://aistudio.google.com/apikey")
        .check_as(
            "google-generative-ai",
            "https://generativelanguage.googleapis.com/v1beta/models",
        ),
    provider("openrouter", "OpenRouter", "OPENROUTER_API_KEY")
        .keys("https://openrouter.ai/settings/keys")
        .check("https://openrouter.ai/api/v1/key")
        .sign_in("OpenRouter"),
    provider("baseten", "Baseten", "BASETEN_API_KEY"),
    provider("cerebras", "Cerebras", "CEREBRAS_API_KEY")
        .keys("https://cloud.cerebras.ai")
        .check("https://api.cerebras.ai/v1/models"),
    provider("deepseek", "DeepSeek", "DEEPSEEK_API_KEY")
        .keys("https://platform.deepseek.com/api_keys")
        .check("https://api.deepseek.com/models"),
    provider("fireworks", "Fireworks", "FIREWORKS_API_KEY")
        .keys("https://app.fireworks.ai/settings/users/api-keys"),
    provider("groq", "Groq", "GROQ_API_KEY")
        .keys("https://console.groq.com/keys")
        .check("https://api.groq.com/openai/v1/models"),
    provider("huggingface", "Hugging Face", "HF_TOKEN")
        .keys("https://huggingface.co/settings/tokens"),
    provider("kimi-coding", "Kimi For Coding", "KIMI_API_KEY").sign_in("Kimi Code"),
    provider("meta", "Meta", "META_API_KEY"),
    provider("minimax", "MiniMax", "MINIMAX_API_KEY"),
    provider("mistral", "Mistral", "MISTRAL_API_KEY")
        .keys("https://console.mistral.ai/api-keys")
        .check("https://api.mistral.ai/v1/models"),
    provider("moonshotai", "Moonshot AI", "MOONSHOT_API_KEY"),
    provider("nvidia", "NVIDIA NIM", "NVIDIA_API_KEY").keys("https://build.nvidia.com"),
    provider("opencode-go", "OpenCode Go", "OPENCODE_API_KEY").keys("https://opencode.ai/auth"),
    provider("opencode", "OpenCode Zen", "OPENCODE_API_KEY").keys("https://opencode.ai/auth"),
    provider("together", "Together AI", "TOGETHER_API_KEY")
        .keys("https://api.together.ai/settings/api-keys"),
    provider(
        "vercel-ai-gateway",
        "Vercel AI Gateway",
        "AI_GATEWAY_API_KEY",
    ),
    provider("xai", "xAI", "XAI_API_KEY")
        .keys("https://console.x.ai")
        .check("https://api.x.ai/v1/models")
        .sign_in("Grok"),
    provider("xiaomi", "Xiaomi MiMo", "XIAOMI_API_KEY"),
    provider("zai", "Z.AI coding plan", "ZAI_API_KEY"),
];

/// The built-in provider called `id`.
pub fn built_in(id: &str) -> Option<&'static BuiltIn> {
    BUILT_IN.iter().find(|provider| provider.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_providers_are_distinct() {
        let mut ids: Vec<&str> = BUILT_IN.iter().map(|provider| provider.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), BUILT_IN.len());
        assert_eq!(built_in("openai").unwrap().label, "OpenAI");
        assert_eq!(built_in("spark"), None);
        // The rest are in the order of their names, to be found.
        let names: Vec<String> = BUILT_IN[BEST_KNOWN..]
            .iter()
            .map(|provider| provider.label.to_lowercase())
            .collect();
        assert!(names.is_sorted(), "{names:?}");
        for provider in BUILT_IN {
            for url in [provider.keys_url, provider.check.map(|check| check.url)] {
                assert!(url.is_none_or(|url| url.starts_with("https://")));
            }
        }
    }
}
