use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub(super) struct AnthropicStreamEvent {
    #[serde(rename = "type")]
    pub(super) kind: String,
    #[serde(default)]
    pub(super) message: Option<AnthropicMessageStart>,
    #[serde(default)]
    pub(super) index: Option<usize>,
    #[serde(default)]
    pub(super) content_block: Option<AnthropicContentBlock>,
    #[serde(default)]
    pub(super) delta: Option<serde_json::Value>,
    #[serde(default)]
    pub(super) usage: Option<AnthropicUsageDelta>,
    #[serde(default)]
    pub(super) error: Option<AnthropicError>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AnthropicMessageStart {
    pub(super) id: String,
    #[serde(default)]
    pub(super) usage: Option<AnthropicMessageUsage>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AnthropicMessageUsage {
    pub(super) input_tokens: i64,
    pub(super) output_tokens: i64,
}

#[derive(Debug, Deserialize)]
pub(super) struct AnthropicUsageDelta {
    pub(super) output_tokens: i64,
}

#[derive(Debug, Deserialize)]
pub(super) struct AnthropicError {
    #[serde(rename = "type")]
    pub(super) kind: String,
    pub(super) message: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum AnthropicContentBlock {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        #[serde(default = "empty_json_object")]
        input: Value,
    },
    ServerToolUse {
        id: String,
        name: String,
        #[serde(default = "empty_json_object")]
        input: Value,
    },
    WebSearchToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: Value,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum AnthropicDelta {
    TextDelta {
        text: String,
    },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta {
        partial_json: String,
    },
    ThinkingDelta {},
    SignatureDelta {},
    #[serde(other)]
    Other,
}

fn empty_json_object() -> Value {
    Value::Object(Default::default())
}
