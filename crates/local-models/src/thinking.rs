//! Thinking with a local model: which of pi's thinking levels it has here, and what pi's
//! `models.json` needs so that they reach its chat template.
//!
//! pi sends thinking to an OpenAI-compatible server as `chat_template_kwargs`
//! (`compat.thinkingFormat: "chat-template"`): `enable_thinking`, and for a template that takes
//! one, a `reasoning_effort` from the model's own values ([`Model::thinking_levels`]); levels
//! without a value are marked unsupported in `thinkingLevelMap`, since a template raises on a value
//! it does not know.

use serde_json::{Map, Value, json};

use crate::catalog::{Engine, Model};

/// pi's thinking levels, lowest first.
pub const LEVELS: [&str; 6] = ["off", "minimal", "low", "medium", "high", "xhigh"];

/// What to write on the model in pi's `models.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct PiThinking {
    /// `compat`: how thinking is sent.
    pub compat: Value,
    /// `thinkingLevelMap`: the value sent for each level, `null` where there is none.
    pub level_map: Value,
}

/// Whether `model` thinks when `engine` runs it: it must, and the engine must pass pi's switch on.
pub fn thinks(model: &Model, engine: &Engine) -> bool {
    model.reasoning == Some(true) && engine.template_kwargs
}

/// The levels pi offers for `model` run by `engine`, lowest first; empty when it does not think.
pub fn levels(model: &Model, engine: &Engine) -> Vec<&'static str> {
    if !thinks(model, engine) {
        return Vec::new();
    }
    if model.thinking_levels.is_empty() {
        return vec!["off", "high"];
    }
    LEVELS
        .into_iter()
        .filter(|level| *level == "off" || model.thinking_levels.contains_key(*level))
        .collect()
}

/// What pi's `models.json` needs on the model; `None` when it does not think here.
pub fn pi_thinking(model: &Model, engine: &Engine) -> Option<PiThinking> {
    if !thinks(model, engine) {
        return None;
    }
    let mut kwargs = Map::new();
    kwargs.insert(
        "enable_thinking".into(),
        json!({"$var": "thinking.enabled"}),
    );
    let mut map = Map::new();
    if model.thinking_levels.is_empty() {
        // On or off: pi's highest level turns it on.
        for level in ["minimal", "low", "medium"] {
            map.insert(level.into(), Value::Null);
        }
    } else {
        let levels = &model.thinking_levels;
        let mut effort = json!({"$var": "thinking.effort"});
        if !levels.contains_key("off") {
            // Off is `enable_thinking: false`; an effort would turn it back on.
            effort["omitWhenOff"] = true.into();
        }
        kwargs.insert("reasoning_effort".into(), effort);
        for level in LEVELS {
            match levels.get(level) {
                Some(value) => {
                    map.insert(level.into(), value.clone().into());
                }
                // pi leaves xhigh out unless it is mapped; off is always there.
                None if level != "off" && level != "xhigh" => {
                    map.insert(level.into(), Value::Null);
                }
                None => {}
            }
        }
    }
    Some(PiThinking {
        compat: json!({"thinkingFormat": "chat-template", "chatTemplateKwargs": kwargs}),
        level_map: Value::Object(map),
    })
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::catalog;

    fn model(id: &str) -> &'static Model {
        catalog().model(id).unwrap()
    }

    fn engine(id: &str) -> &'static Engine {
        catalog().engine(id).unwrap()
    }

    #[test]
    fn a_switch_is_off_or_on() {
        let (qwen, llama) = (model("qwen3.5-9b"), engine("llama-cpp"));
        assert_eq!(levels(qwen, llama), ["off", "high"]);
        let pi = pi_thinking(qwen, llama).unwrap();
        assert_eq!(
            pi.compat,
            json!({"thinkingFormat": "chat-template",
                   "chatTemplateKwargs": {"enable_thinking": {"$var": "thinking.enabled"}}})
        );
        assert_eq!(
            pi.level_map,
            json!({"minimal": null, "low": null, "medium": null})
        );
    }

    #[test]
    fn efforts_are_the_templates_own() {
        let llama = engine("llama-cpp");
        let bonsai = model("ternary-bonsai-2-27b");
        assert_eq!(levels(bonsai, llama), ["off", "low", "medium", "high"]);
        let pi = pi_thinking(bonsai, llama).unwrap();
        assert_eq!(
            pi.compat["chatTemplateKwargs"]["reasoning_effort"],
            json!({"$var": "thinking.effort", "omitWhenOff": true})
        );
        assert_eq!(
            pi.level_map,
            json!({"minimal": null, "low": "low", "medium": "medium", "high": "xhigh"})
        );
        // Off is an effort of its own.
        let inkling = pi_thinking(model("inkling-2-mini"), llama).unwrap();
        assert_eq!(
            inkling.compat["chatTemplateKwargs"]["reasoning_effort"],
            json!({"$var": "thinking.effort"})
        );
        assert_eq!(inkling.level_map["off"], "none");
        assert_eq!(inkling.level_map["xhigh"], "xhigh");
    }

    #[test]
    fn no_thinking_where_the_engine_cannot_pass_it_on() {
        let gemma = model("gemma-4-26b-a4b");
        let turbo = engine("turbo-fieldfare");
        assert!(!thinks(gemma, turbo));
        assert_eq!(levels(gemma, turbo), Vec::<&str>::new());
        assert_eq!(pi_thinking(gemma, turbo), None);
    }
}
