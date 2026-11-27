//! How a model server takes pi's thinking levels, found out by asking it, so that every level
//! the Models dialog sets up does what it says.
//!
//! Servers differ, and so do versions of one: vLLM and current llama.cpp read a top-level
//! `reasoning_effort` (`"none"` turns thinking off), older llama.cpp only reads
//! `chat_template_kwargs`, Ollama maps `reasoning_effort: "none"` to no thinking, and a model's
//! chat template decides which efforts change anything at all. Unset, pi sends nothing for "off",
//! so the template's default applies, which for most thinking models is to think.
//!
//! So instead of guessing from the program's name, the check looks at what the server does with
//! each form, cheaply:
//!
//! - vLLM renders the prompt of a one-token request back (`prompt_logprobs`): one request per
//!   form shows which ones change the prompt, without generating anything ([`renders`]).
//! - llama.cpp's `/props` lists the efforts its template renders distinctly and those it rejects
//!   ([`PROPS`]).
//! - One short request with thinking off shows whether "off" really stops it ([`off_trial`]):
//!   some models think a little at any effort.
//!
//! [`setup`] turns the answers into what pi's `models.json` needs. Sends nothing itself.

use serde_json::{Map, Value, json};

use crate::server::{Kind, root};

/// pi's levels above off, lowest first.
pub const LEVELS: [&str; 5] = ["minimal", "low", "medium", "high", "xhigh"];

/// A way of asking for thinking whose rendered prompt is compared with the default's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// Nothing about thinking.
    Default,
    /// `reasoning_effort: "none"`.
    OffEffort,
    /// `chat_template_kwargs: {"enable_thinking": false}`.
    OffSwitch,
    /// `reasoning_effort: <LEVELS[i]>`.
    Effort(usize),
}

/// A request whose answer is a rendered prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Render {
    pub variant: Variant,
    pub url: String,
    pub body: String,
}

/// llama.cpp's server settings, which include its template's capabilities.
pub const PROPS: &str = "/props";

/// The requests that have a vLLM-like server render its prompt for each form: one-token replies
/// that echo the prompt as `prompt_logprobs`. Other servers ignore the field, and the check
/// moves on.
pub fn renders(base_url: &str, model: &str) -> Vec<Render> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let variants = [Variant::Default, Variant::OffEffort, Variant::OffSwitch]
        .into_iter()
        .chain((0..LEVELS.len()).map(Variant::Effort));
    variants
        .map(|variant| {
            let mut body = json!({
                "model": model,
                "messages": [{ "role": "user", "content": "hi" }],
                "max_tokens": 1,
                "prompt_logprobs": 0,
            });
            match variant {
                Variant::Default => {}
                Variant::OffEffort => body["reasoning_effort"] = "none".into(),
                Variant::OffSwitch => {
                    body["chat_template_kwargs"] = json!({ "enable_thinking": false });
                }
                Variant::Effort(level) => body["reasoning_effort"] = LEVELS[level].into(),
            }
            Render {
                variant,
                url: url.clone(),
                body: body.to_string(),
            }
        })
        .collect()
}

/// The URL of llama.cpp's settings for a server at `base_url`.
pub fn props_url(base_url: &str) -> String {
    format!("{}{PROPS}", root(base_url))
}

/// Ollama's details of `model` (its family among them): the URL and body to POST.
pub fn ollama_show(base_url: &str, model: &str) -> (String, String) {
    (
        format!("{}/api/show", root(base_url)),
        json!({ "model": model }).to_string(),
    )
}

/// The model family in Ollama's details ("gptoss", "qwen3", …).
pub fn family(body: &str) -> Option<String> {
    let answer: Value = serde_json::from_str(body).ok()?;
    let details = answer.get("details")?;
    details
        .get("family")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The prompt a server rendered, from its answer to a [`renders`] request; `None` when it did
/// not echo it.
pub fn rendered_prompt(body: &str) -> Option<String> {
    let answer: Value = serde_json::from_str(body).ok()?;
    let tokens = answer.get("prompt_logprobs")?.as_array()?;
    let text: String = tokens
        .iter()
        .filter_map(|token| {
            token
                .as_object()?
                .values()
                .next()?
                .get("decoded_token")?
                .as_str()
        })
        .collect();
    (!text.is_empty()).then_some(text)
}

/// What was found out about a model's server.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    /// Rendered prompts, by form; empty when the server does not render them.
    pub renders: Vec<(Variant, String)>,
    /// llama.cpp's `/props`.
    pub props: Option<Value>,
    /// The model's family, where the server says (Ollama).
    pub family: Option<String>,
}

/// What pi's `models.json` needs on the model.
#[derive(Debug, Clone, PartialEq)]
pub struct Setup {
    /// Merged into the model's `compat` (`null`: that key goes).
    pub compat: Value,
    /// Its `thinkingLevelMap`.
    pub level_map: Value,
    /// How "off" is sent; `None` when the server has no way to turn thinking off.
    pub off: Option<Off>,
}

/// How a server is asked not to think.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Off {
    /// A top-level `reasoning_effort` with this value.
    Effort(String),
    /// `chat_template_kwargs: {"enable_thinking": false}`.
    Switch,
}

/// The setup for a thinking model on a server of `kind`, from what was found out.
pub fn setup(kind: Kind, facts: &Facts) -> Setup {
    if let Some(setup) = from_renders(&facts.renders) {
        return setup;
    }
    if let Some(caps) = facts
        .props
        .as_ref()
        .and_then(|props| props.get("chat_template_caps"))
    {
        return from_template_caps(caps);
    }
    if facts.props.is_some() {
        // A llama.cpp too old to say: it reads `chat_template_kwargs` only.
        return on_off_switch();
    }
    match kind {
        // Ollama turns `reasoning_effort: "none"` into no thinking, and its other values into
        // levels for the models that have them (gpt-oss).
        Kind::Ollama
            if facts
                .family
                .as_deref()
                .is_some_and(|f| f.contains("gptoss")) =>
        {
            effort_setup(&["minimal", "low", "medium", "high"].map(|level| {
                let value = if level == "minimal" { "low" } else { level };
                (level, value.to_owned())
            }))
        }
        // Elsewhere only on and off are known to work.
        _ => on_off_effort(),
    }
}

/// From rendered prompts: which forms change the prompt.
fn from_renders(renders: &[(Variant, String)]) -> Option<Setup> {
    let rendered = |variant: Variant| {
        renders
            .iter()
            .find(|(found, _)| *found == variant)
            .map(|(_, text)| text.as_str())
    };
    let default = rendered(Variant::Default)?;
    let changes = |variant| rendered(variant).is_some_and(|text| text != default);
    let efforts: Vec<Option<&str>> = (0..LEVELS.len())
        .map(|level| rendered(Variant::Effort(level)))
        .collect();
    let any_effort = (0..LEVELS.len()).any(|level| changes(Variant::Effort(level)));
    if changes(Variant::OffEffort) || any_effort {
        // Levels whose prompts are the same do the same: they send one value, the highest
        // level's (high, where it is among them), and the picker shows them as one.
        let mut pairs = Vec::new();
        let mut start = 0;
        while start < LEVELS.len() {
            let mut end = start + 1;
            while end < LEVELS.len() && efforts[end] == efforts[start] {
                end += 1;
            }
            let run = &LEVELS[start..end];
            let value = if run.contains(&"high") {
                "high"
            } else {
                run[run.len() - 1]
            };
            pairs.extend(run.iter().map(|level| (*level, value.to_owned())));
            start = end;
        }
        let mut setup = effort_setup(&pairs);
        if !changes(Variant::OffEffort) {
            setup.off = None;
            setup.level_map.as_object_mut()?.remove("off");
        }
        return Some(setup);
    }
    if changes(Variant::OffSwitch) {
        return Some(on_off_switch());
    }
    // The template takes no thinking control: offer it as it is, on.
    let mut setup = on_off_effort();
    setup.off = None;
    setup.level_map.as_object_mut()?.remove("off");
    Some(setup)
}

/// From llama.cpp's template capabilities: the efforts it renders distinctly.
fn from_template_caps(caps: &Value) -> Setup {
    let names = |key: &str| -> Vec<String> {
        caps.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect()
    };
    let levels = names("reasoning_effort_levels");
    if levels.is_empty() {
        return on_off_switch();
    }
    // Each of pi's levels sends the nearest effort the template renders, at or below it (the
    // lowest one for those below them all); "none" is not a level of thinking.
    let distinct: Vec<&str> = levels
        .iter()
        .map(String::as_str)
        .filter(|level| *level != "none")
        .collect();
    let order = |level: &str| {
        ["minimal", "low", "medium", "high", "xhigh", "max"]
            .iter()
            .position(|known| *known == level)
    };
    let pairs: Vec<(&str, String)> = LEVELS
        .iter()
        .filter_map(|level| {
            let rank = order(level)?;
            let value = distinct
                .iter()
                .rev()
                .find(|found| order(found).is_some_and(|found| found <= rank))
                .or_else(|| distinct.first())?;
            Some((*level, (*value).to_owned()))
        })
        .collect();
    let mut level_map = Map::new();
    level_map.insert("off".into(), "none".into());
    for (level, value) in &pairs {
        level_map.insert((*level).into(), value.clone().into());
    }
    Setup {
        compat: json!({
            "thinkingFormat": "chat-template",
            "chatTemplateKwargs": {
                "enable_thinking": { "$var": "thinking.enabled" },
                "reasoning_effort": { "$var": "thinking.effort", "omitWhenOff": true },
            },
        }),
        level_map: Value::Object(level_map),
        off: Some(Off::Switch),
    }
}

/// A top-level `reasoning_effort` for each level, `"none"` for off.
fn effort_setup(pairs: &[(&str, String)]) -> Setup {
    let mut level_map = Map::new();
    level_map.insert("off".into(), "none".into());
    for (level, value) in pairs {
        level_map.insert((*level).into(), value.clone().into());
    }
    Setup {
        // Merged into the model's own: a template's kwargs from before go.
        compat: json!({
            "thinkingFormat": "openai",
            "supportsReasoningEffort": true,
            "chatTemplateKwargs": null,
        }),
        level_map: Value::Object(level_map),
        off: Some(Off::Effort("none".to_owned())),
    }
}

/// pi's levels for a model that only thinks or doesn't: they all send "high", and the picker
/// shows one of them. Without xhigh, which pi only offers when it is mapped.
const ON: [&str; 4] = ["minimal", "low", "medium", "high"];

/// Thinking on or off by `reasoning_effort`.
fn on_off_effort() -> Setup {
    effort_setup(&ON.map(|level| (level, "high".to_owned())))
}

/// Thinking on or off by the template's `enable_thinking`.
fn on_off_switch() -> Setup {
    let mut setup = on_off_effort();
    setup.compat = json!({
        "thinkingFormat": "chat-template",
        "chatTemplateKwargs": { "enable_thinking": { "$var": "thinking.enabled" } },
    });
    setup.off = Some(Off::Switch);
    setup
}

/// A short request with thinking off, to see whether the model thinks anyway: the URL and body.
pub fn off_trial(base_url: &str, model: &str, off: &Off) -> (String, String) {
    let mut body = json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Say hi." }],
        "max_tokens": 48,
    });
    match off {
        Off::Effort(value) => body["reasoning_effort"] = value.as_str().into(),
        Off::Switch => body["chat_template_kwargs"] = json!({ "enable_thinking": false }),
    }
    (
        format!("{}/chat/completions", base_url.trim_end_matches('/')),
        body.to_string(),
    )
}

/// Whether an answer to [`off_trial`] thought: reasoning beside the text, or thinking tags in it.
pub fn thought(body: &str) -> Option<bool> {
    let answer: Value = serde_json::from_str(body).ok()?;
    let message = answer.pointer("/choices/0/message")?;
    let reasoning = ["reasoning_content", "reasoning", "thinking"]
        .iter()
        .any(|key| {
            message
                .get(*key)
                .and_then(Value::as_str)
                .is_some_and(|text| !text.trim().is_empty())
        });
    let content = message.get("content").and_then(Value::as_str).unwrap_or("");
    Some(reasoning || content.contains("<think>") || content.contains("<|channel|>analysis"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn renders_of(effort: [&str; 5], off_effort: &str, off_switch: &str) -> Vec<(Variant, String)> {
        let mut renders = vec![
            (Variant::Default, "level 0.9".to_owned()),
            (Variant::OffEffort, off_effort.to_owned()),
            (Variant::OffSwitch, off_switch.to_owned()),
        ];
        renders.extend(
            effort
                .iter()
                .enumerate()
                .map(|(i, text)| (Variant::Effort(i), (*text).to_owned())),
        );
        renders
    }

    #[test]
    fn efforts_the_template_renders_are_kept_and_the_same_ones_merged() {
        // Like Inkling on vLLM: each effort its own value.
        let facts = Facts {
            renders: renders_of(["0.1", "0.2", "0.7", "level 0.9", "0.99"], "0", "level 0.9"),
            ..Facts::default()
        };
        let found = setup(Kind::Vllm, &facts);
        assert_eq!(
            found.level_map,
            json!({"off": "none", "minimal": "minimal", "low": "low", "medium": "medium",
                   "high": "high", "xhigh": "xhigh"})
        );
        assert_eq!(found.off, Some(Off::Effort("none".into())));

        // Like Qwen3 on vLLM: any effort turns thinking on, "none" turns it off.
        let on = "level 0.9";
        let facts = Facts {
            renders: renders_of([on; 5], "no think", "no think"),
            ..Facts::default()
        };
        let found = setup(Kind::Vllm, &facts);
        assert_eq!(
            found.level_map,
            json!({"off": "none", "minimal": "high", "low": "high", "medium": "high",
                   "high": "high", "xhigh": "high"})
        );
    }

    #[test]
    fn a_template_that_only_has_a_switch_uses_it() {
        let same = "level 0.9";
        let facts = Facts {
            renders: renders_of([same; 5], same, "no think"),
            ..Facts::default()
        };
        let found = setup(Kind::Vllm, &facts);
        assert_eq!(found.off, Some(Off::Switch));
        assert_eq!(found.compat["thinkingFormat"], "chat-template");

        // Nothing changes the prompt: no off to offer.
        let facts = Facts {
            renders: renders_of([same; 5], same, same),
            ..Facts::default()
        };
        let found = setup(Kind::Vllm, &facts);
        assert_eq!(found.off, None);
        assert_eq!(found.level_map.get("off"), None);
    }

    #[test]
    fn llama_cpp_uses_the_efforts_its_template_renders() {
        let facts = Facts {
            props: Some(json!({"chat_template_caps": {
                "reasoning_effort_levels": ["none", "low", "medium", "high"],
                "reasoning_effort_rejected": ["xhigh"]}})),
            ..Facts::default()
        };
        let found = setup(Kind::LlamaCpp, &facts);
        assert_eq!(
            found.level_map,
            json!({"off": "none", "minimal": "low", "low": "low", "medium": "medium",
                   "high": "high", "xhigh": "high"})
        );
        assert_eq!(found.compat["thinkingFormat"], "chat-template");

        // Older builds say nothing of their template: the switch.
        let facts = Facts {
            props: Some(json!({"default_generation_settings": {}})),
            ..Facts::default()
        };
        assert_eq!(setup_off(&facts, Kind::LlamaCpp), Some(Off::Switch));
    }

    fn setup_off(facts: &Facts, kind: Kind) -> Option<Off> {
        setup(kind, facts).off
    }

    #[test]
    fn ollama_and_others_turn_thinking_off_with_none() {
        let gpt_oss = Facts {
            family: Some("gptoss".into()),
            ..Facts::default()
        };
        assert_eq!(
            setup(Kind::Ollama, &gpt_oss).level_map,
            json!({"off": "none", "minimal": "low", "low": "low", "medium": "medium", "high": "high"})
        );
        let other = setup(Kind::LmStudio, &Facts::default());
        assert_eq!(other.off, Some(Off::Effort("none".into())));
        assert_eq!(
            other.level_map,
            json!({"off": "none", "minimal": "high", "low": "high", "medium": "high", "high": "high"})
        );
    }

    #[test]
    fn reads_rendered_prompts_and_whether_a_reply_thought() {
        let body = r#"{"prompt_logprobs": [null, {"5": {"decoded_token": "<|user|>"}}, {"9": {"decoded_token": "hi"}}]}"#;
        assert_eq!(rendered_prompt(body).as_deref(), Some("<|user|>hi"));
        assert_eq!(rendered_prompt(r#"{"choices": []}"#), None);

        let reply = |message: Value| json!({"choices": [{"message": message}]}).to_string();
        assert_eq!(
            thought(&reply(
                json!({"content": "Hi!", "reasoning_content": "The user"})
            )),
            Some(true)
        );
        assert_eq!(
            thought(&reply(json!({"content": "Hi!", "reasoning": null}))),
            Some(false)
        );
        assert_eq!(thought(&reply(json!({"content": "<think>hm"}))), Some(true));
        assert_eq!(thought("not json"), None);

        let (url, body) = off_trial("http://h:1/v1/", "m", &Off::Switch);
        assert_eq!(url, "http://h:1/v1/chat/completions");
        assert!(body.contains(r#""enable_thinking":false"#), "{body}");
        assert_eq!(renders("http://h:1/v1", "m").len(), 8);
    }
}
