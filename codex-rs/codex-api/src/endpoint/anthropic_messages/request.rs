use crate::common::ResponsesApiRequest;
use crate::error::ApiError;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use serde::Serialize;
use serde_json::Value;

#[cfg(test)]
const TEST_ANTHROPIC_MAX_TOKENS: u32 = 4096;

#[derive(Debug, Serialize, PartialEq)]
struct AnthropicMessagesRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    system: String,
}

#[derive(Debug, Serialize, PartialEq)]
struct AnthropicMessage {
    role: AnthropicRole,
    content: Vec<AnthropicContentBlock>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum AnthropicRole {
    User,
    Assistant,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlock {
    Text { text: String },
}

pub(super) fn anthropic_messages_request(request: ResponsesApiRequest) -> Result<Value, ApiError> {
    validate_text_only_request(&request)?;
    let ResponsesApiRequest {
        model,
        instructions,
        input,
        max_output_tokens,
        ..
    } = request;
    let max_tokens = max_output_tokens.ok_or_else(|| ApiError::InvalidRequest {
        message: "anthropic_messages requires max_output_tokens from model metadata".to_string(),
    })?;
    let AnthropicConvertedMessages {
        messages,
        system_messages,
    } = anthropic_messages_from_items(input)?;

    let request = AnthropicMessagesRequest {
        model,
        max_tokens,
        messages,
        stream: true,
        system: anthropic_system_prompt(instructions, system_messages),
    };

    serde_json::to_value(request)
        .map_err(|err| ApiError::Stream(format!("failed to encode Anthropic request: {err}")))
}

fn validate_text_only_request(request: &ResponsesApiRequest) -> Result<(), ApiError> {
    if !request.tools.is_empty() {
        return Err(text_only_request_error(
            "anthropic_messages text-only support does not accept tools",
        ));
    }

    if request.tool_choice != "auto" {
        return Err(text_only_request_error(
            "anthropic_messages text-only support only accepts auto tool_choice",
        ));
    }

    if request.parallel_tool_calls {
        return Err(text_only_request_error(
            "anthropic_messages text-only support does not accept parallel tool calls",
        ));
    }

    if request.reasoning.is_some() || !request.include.is_empty() {
        return Err(text_only_request_error(
            "anthropic_messages text-only support does not accept reasoning controls",
        ));
    }

    if request.text.is_some() {
        return Err(text_only_request_error(
            "anthropic_messages text-only support does not accept OpenAI text controls",
        ));
    }

    if request.store {
        return Err(text_only_request_error(
            "anthropic_messages text-only support does not accept store",
        ));
    }

    if !request.stream {
        return Err(text_only_request_error(
            "anthropic_messages requires streaming requests",
        ));
    }

    Ok(())
}

#[cfg(test)]
fn anthropic_messages_body(request: ResponsesApiRequest) -> Result<Value, ApiError> {
    anthropic_messages_request(request)
}

fn text_only_request_error(message: &str) -> ApiError {
    ApiError::InvalidRequest {
        message: message.to_string(),
    }
}

fn anthropic_messages_from_items(
    items: Vec<ResponseItem>,
) -> Result<AnthropicConvertedMessages, ApiError> {
    let mut messages: Vec<AnthropicMessage> = Vec::new();
    let mut system_messages = Vec::new();
    for item in items {
        match anthropic_message_from_item(item)? {
            AnthropicConvertedItem::Message(message) => {
                if let Some(previous) = messages.last_mut()
                    && previous.role == message.role
                {
                    previous.content.extend(message.content);
                    continue;
                }

                messages.push(message);
            }
            AnthropicConvertedItem::System(text) => system_messages.push(text),
            AnthropicConvertedItem::Skip => {}
        };
    }

    if messages.is_empty() {
        return Err(ApiError::InvalidRequest {
            message: "anthropic_messages requires at least one text message".to_string(),
        });
    }

    Ok(AnthropicConvertedMessages {
        messages,
        system_messages,
    })
}

#[derive(Debug, PartialEq)]
struct AnthropicConvertedMessages {
    messages: Vec<AnthropicMessage>,
    system_messages: Vec<String>,
}

enum AnthropicConvertedItem {
    Message(AnthropicMessage),
    System(String),
    Skip,
}

fn anthropic_message_from_item(item: ResponseItem) -> Result<AnthropicConvertedItem, ApiError> {
    match item {
        ResponseItem::Message { role, content, .. } => {
            let role = match role.as_str() {
                "user" => AnthropicRole::User,
                "assistant" => AnthropicRole::Assistant,
                "developer" | "system" => {
                    return Ok(AnthropicConvertedItem::System(anthropic_system_content(
                        content,
                    )?));
                }
                other => {
                    return Err(ApiError::InvalidRequest {
                        message: format!(
                            "anthropic_messages does not support message role `{other}`"
                        ),
                    });
                }
            };
            let content = anthropic_content_blocks(content)?;
            if content.is_empty() {
                return Err(ApiError::InvalidRequest {
                    message: "anthropic_messages requires message text content".to_string(),
                });
            }

            Ok(AnthropicConvertedItem::Message(AnthropicMessage {
                role,
                content,
            }))
        }
        ResponseItem::Reasoning { .. }
        | ResponseItem::Compaction { .. }
        | ResponseItem::CompactionTrigger
        | ResponseItem::ContextCompaction { .. }
        | ResponseItem::Other => Ok(AnthropicConvertedItem::Skip),
        ResponseItem::LocalShellCall { .. }
        | ResponseItem::FunctionCall { .. }
        | ResponseItem::ToolSearchCall { .. }
        | ResponseItem::FunctionCallOutput { .. }
        | ResponseItem::CustomToolCall { .. }
        | ResponseItem::CustomToolCallOutput { .. }
        | ResponseItem::ToolSearchOutput { .. }
        | ResponseItem::WebSearchCall { .. }
        | ResponseItem::ImageGenerationCall { .. } => Err(ApiError::InvalidRequest {
            message: "anthropic_messages text-only support cannot replay tool or hosted-call items"
                .to_string(),
        }),
    }
}

fn anthropic_content_blocks(
    content: Vec<ContentItem>,
) -> Result<Vec<AnthropicContentBlock>, ApiError> {
    let mut blocks = Vec::new();
    for item in content {
        match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                blocks.push(AnthropicContentBlock::Text { text });
            }
            ContentItem::InputImage { .. } => {
                return Err(ApiError::InvalidRequest {
                    message: "anthropic_messages text-only support does not accept images"
                        .to_string(),
                });
            }
        }
    }

    Ok(blocks)
}

fn anthropic_system_content(content: Vec<ContentItem>) -> Result<String, ApiError> {
    let mut text = Vec::new();
    for item in content {
        match item {
            ContentItem::InputText { text: content }
            | ContentItem::OutputText { text: content } => text.push(content),
            ContentItem::InputImage { .. } => {
                return Err(ApiError::InvalidRequest {
                    message: "anthropic_messages system content supports only text".to_string(),
                });
            }
        }
    }

    if text.is_empty() {
        return Err(ApiError::InvalidRequest {
            message: "anthropic_messages requires system text content".to_string(),
        });
    }

    Ok(text.join("\n"))
}

fn anthropic_system_prompt(instructions: String, system_messages: Vec<String>) -> String {
    let mut parts = Vec::new();
    if !instructions.is_empty() {
        parts.push(instructions);
    }
    parts.extend(system_messages.into_iter().filter(|text| !text.is_empty()));
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn text_message(role: &str, text: &str) -> ResponseItem {
        ResponseItem::Message {
            id: None,
            role: role.to_string(),
            content: vec![ContentItem::InputText {
                text: text.to_string(),
            }],
            phase: None,
        }
    }

    fn text_request() -> ResponsesApiRequest {
        ResponsesApiRequest {
            model: "claude-sonnet-4-6".to_string(),
            instructions: "system".to_string(),
            input: vec![text_message("user", "hello")],
            tools: Vec::new(),
            tool_choice: "auto".to_string(),
            parallel_tool_calls: false,
            max_output_tokens: Some(TEST_ANTHROPIC_MAX_TOKENS),
            reasoning: None,
            store: false,
            stream: true,
            include: Vec::new(),
            service_tier: None,
            prompt_cache_key: None,
            text: None,
            client_metadata: None,
        }
    }

    #[test]
    fn converts_text_messages_to_anthropic_body() {
        let request = text_request();

        let body = anthropic_messages_body(request).expect("request should convert");

        assert_eq!(
            body,
            serde_json::json!({
                "model": "claude-sonnet-4-6",
                "max_tokens": TEST_ANTHROPIC_MAX_TOKENS,
                "messages": [{
                    "role": "user",
                    "content": [{"type": "text", "text": "hello"}]
                }],
                "stream": true,
                "system": "system"
            })
        );
    }

    #[test]
    fn converts_developer_messages_to_anthropic_system_prompt() {
        let mut request = text_request();
        request.input = vec![
            text_message("developer", "developer instructions"),
            text_message("user", "hello"),
        ];

        let body = anthropic_messages_body(request).expect("developer message should convert");

        assert_eq!(
            body["system"].as_str(),
            Some("system\n\ndeveloper instructions")
        );
        assert_eq!(body["messages"].as_array().map(Vec::len), Some(1));
        assert_eq!(body["messages"][0]["role"].as_str(), Some("user"));
    }

    #[test]
    fn rejects_tools_in_text_only_phase() {
        let mut request = text_request();
        request.tools = vec![serde_json::json!({
            "type": "function",
            "name": "shell",
            "description": "Run a command.",
            "parameters": {"type": "object"}
        })];

        let error = anthropic_messages_body(request).expect_err("tools should be rejected");

        assert!(
            matches!(error, ApiError::InvalidRequest { ref message } if message.contains("does not accept tools")),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn rejects_missing_max_output_tokens() {
        let mut request = text_request();
        request.max_output_tokens = None;

        let error =
            anthropic_messages_body(request).expect_err("missing max_output_tokens should fail");

        assert!(
            matches!(error, ApiError::InvalidRequest { ref message } if message.contains("max_output_tokens")),
            "unexpected error: {error:?}"
        );
    }

    #[test]
    fn rejects_tool_history_in_text_only_phase() {
        let err = anthropic_messages_from_items(vec![ResponseItem::FunctionCall {
            id: None,
            name: "shell".to_string(),
            namespace: None,
            arguments: "{}".to_string(),
            call_id: "call-1".to_string(),
        }])
        .expect_err("tool history should be rejected");

        assert!(matches!(err, ApiError::InvalidRequest { .. }));
    }
}
