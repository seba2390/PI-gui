//! Extension UI subprotocol: dialogs and fire-and-forget UI updates.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyKind {
    #[default]
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WidgetPlacement {
    #[default]
    AboveEditor,
    BelowEditor,
}

/// `extension_ui_request` (the `id` lives on [`ExtensionUiRequest`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all_fields = "camelCase")]
pub enum ExtensionUiMethod {
    #[serde(rename = "select")]
    Select {
        title: String,
        options: Vec<String>,
        #[serde(default)]
        timeout: Option<u64>,
    },
    #[serde(rename = "confirm")]
    Confirm {
        title: String,
        #[serde(default)]
        message: String,
        #[serde(default)]
        timeout: Option<u64>,
    },
    #[serde(rename = "input")]
    Input {
        title: String,
        #[serde(default)]
        placeholder: Option<String>,
        #[serde(default)]
        timeout: Option<u64>,
    },
    #[serde(rename = "editor")]
    Editor {
        title: String,
        #[serde(default)]
        prefill: Option<String>,
    },
    #[serde(rename = "notify")]
    Notify {
        message: String,
        #[serde(default)]
        notify_type: NotifyKind,
    },
    #[serde(rename = "setStatus")]
    SetStatus {
        status_key: String,
        #[serde(default)]
        status_text: Option<String>,
    },
    #[serde(rename = "setWidget")]
    SetWidget {
        widget_key: String,
        #[serde(default)]
        widget_lines: Option<Vec<String>>,
        #[serde(default)]
        widget_placement: WidgetPlacement,
    },
    #[serde(rename = "setTitle")]
    SetTitle { title: String },
    #[serde(rename = "set_editor_text")]
    SetEditorText { text: String },
    #[serde(other)]
    Unknown,
}

impl ExtensionUiMethod {
    /// Dialog methods block the extension until a response is sent.
    pub fn expects_response(&self) -> bool {
        matches!(
            self,
            Self::Select { .. } | Self::Confirm { .. } | Self::Input { .. } | Self::Editor { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtensionUiRequest {
    pub id: String,
    #[serde(flatten)]
    pub method: ExtensionUiMethod,
}

/// The client's answer to a dialog request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionUiAnswer {
    Value(String),
    Confirmed(bool),
    Cancelled,
}

#[derive(Serialize)]
pub(crate) struct ExtensionUiResponse<'a> {
    pub r#type: &'static str,
    pub id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancelled: Option<bool>,
}

impl<'a> ExtensionUiResponse<'a> {
    pub fn new(id: &'a str, answer: &'a ExtensionUiAnswer) -> Self {
        let (value, confirmed, cancelled) = match answer {
            ExtensionUiAnswer::Value(value) => (Some(value.as_str()), None, None),
            ExtensionUiAnswer::Confirmed(confirmed) => (None, Some(*confirmed), None),
            ExtensionUiAnswer::Cancelled => (None, None, Some(true)),
        };
        Self {
            r#type: "extension_ui_response",
            id,
            value,
            confirmed,
            cancelled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn decodes_dialog_and_fire_and_forget_requests() {
        let select: ExtensionUiRequest = serde_json::from_value(json!({
            "type":"extension_ui_request","id":"u1","method":"select",
            "title":"Allow?","options":["Allow","Block"],"timeout":10000
        }))
        .unwrap();
        assert_eq!(select.id, "u1");
        assert!(select.method.expects_response());

        let status: ExtensionUiRequest = serde_json::from_value(json!({
            "type":"extension_ui_request","id":"u2","method":"setStatus","statusKey":"k"
        }))
        .unwrap();
        assert_eq!(
            status.method,
            ExtensionUiMethod::SetStatus {
                status_key: "k".into(),
                status_text: None
            }
        );
        assert!(!status.method.expects_response());
    }

    #[test]
    fn encodes_answers() {
        let answer = ExtensionUiAnswer::Confirmed(true);
        assert_eq!(
            serde_json::to_value(ExtensionUiResponse::new("u1", &answer)).unwrap(),
            json!({"type":"extension_ui_response","id":"u1","confirmed":true})
        );
        let answer = ExtensionUiAnswer::Cancelled;
        assert_eq!(
            serde_json::to_value(ExtensionUiResponse::new("u1", &answer)).unwrap(),
            json!({"type":"extension_ui_response","id":"u1","cancelled":true})
        );
    }
}
