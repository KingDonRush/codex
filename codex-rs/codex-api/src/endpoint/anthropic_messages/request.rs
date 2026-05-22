mod tools;

use crate::anthropic_tool_names::AnthropicToolNameMap;
use crate::common::ResponsesApiRequest;
use crate::endpoint::anthropic_messages::request::tools::AnthropicConvertedTools;
use crate::endpoint::anthropic_messages::request::tools::anthropic_tools_from_responses_api_tools;
use crate::endpoint::anthropic_messages::request::tools::anthropic_tools_from_tool_search_outputs;
use crate::error::ApiError;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;
use serde::Serialize;
use serde_json::Value;

#[cfg(test)]
const TEST_ANTHROPIC_MAX_TOKENS: u32 = 4096;
const INTERNAL_TOOL_SEARCH_NAME: &str = "tool_search";
const ANTHROPIC_TOOL_SEARCH_NAME: &str = "codex_tool_search";

#[derive(Debug, PartialEq)]
pub(super) struct AnthropicEncodedRequest {
    pub(super) body: Value,
    pub(super) tool_name_map: AnthropicToolNameMap,
}

#[derive(Debug, Serialize, PartialEq)]
struct AnthropicMessagesRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<AnthropicTool>,
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
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(untagged)]
enum AnthropicTool {
    Custom(AnthropicCustomTool),
}

#[derive(Debug, Serialize, PartialEq)]
struct AnthropicCustomTool {
    name: String,
    description: String,
    input_schema: Value,
}

pub(super) fn anthropic_messages_request(
    request: ResponsesApiRequest,
) -> Result<AnthropicEncodedRequest, ApiError> {
    validate_text_only_request(&request)?;
    let ResponsesApiRequest {
        model,
        instructions,
        input,
        tools,
        max_output_tokens,
        ..
    } = request;
    let max_tokens = max_output_tokens.ok_or_else(|| ApiError::InvalidRequest {
        message: "anthropic_messages requires max_output_tokens from model metadata".to_string(),
    })?;
    let mut converted_tools = anthropic_tools_from_responses_api_tools(tools)?;
    anthropic_tools_from_tool_search_outputs(&input, &mut converted_tools)?;
    let AnthropicConvertedTools {
        tools,
        tool_name_map,
    } = converted_tools;
    let AnthropicConvertedMessages {
        messages,
        system_messages,
    } = anthropic_messages_from_items(input, &tool_name_map)?;

    let request = AnthropicMessagesRequest {
        model,
        max_tokens,
        messages,
        stream: true,
        tools,
        system: anthropic_system_prompt(instructions, system_messages),
    };

    let body = serde_json::to_value(request)
        .map_err(|err| ApiError::Stream(format!("failed to encode Anthropic request: {err}")))?;

    Ok(AnthropicEncodedRequest {
        body,
        tool_name_map,
    })
}

fn validate_text_only_request(request: &ResponsesApiRequest) -> Result<(), ApiError> {
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
    Ok(anthropic_messages_request(request)?.body)
}

fn text_only_request_error(message: &str) -> ApiError {
    ApiError::InvalidRequest {
        message: message.to_string(),
    }
}

fn anthropic_messages_from_items(
    items: Vec<ResponseItem>,
    tool_name_map: &AnthropicToolNameMap,
) -> Result<AnthropicConvertedMessages, ApiError> {
    let mut messages: Vec<AnthropicMessage> = Vec::new();
    let mut system_messages = Vec::new();
    for item in items {
        match anthropic_message_from_item(item, tool_name_map)? {
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

fn anthropic_message_from_item(
    item: ResponseItem,
    tool_name_map: &AnthropicToolNameMap,
) -> Result<AnthropicConvertedItem, ApiError> {
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
        ResponseItem::FunctionCall {
            name,
            namespace,
            arguments,
            call_id,
            ..
        } => {
            let name = tool_name_map.anthropic_name(namespace.as_deref(), &name);
            Ok(AnthropicConvertedItem::Message(AnthropicMessage {
                role: AnthropicRole::Assistant,
                content: vec![AnthropicContentBlock::ToolUse {
                    id: call_id,
                    name,
                    input: anthropic_tool_input(&arguments)?,
                }],
            }))
        }
        ResponseItem::FunctionCallOutput { call_id, output } => {
            Ok(AnthropicConvertedItem::Message(AnthropicMessage {
                role: AnthropicRole::User,
                content: vec![AnthropicContentBlock::ToolResult {
                    tool_use_id: call_id,
                    content: anthropic_tool_result_content(output)?,
                }],
            }))
        }
        ResponseItem::ToolSearchCall {
            call_id: Some(call_id),
            execution,
            arguments,
            ..
        } if execution == "client" => {
            let name = tool_name_map.anthropic_name(None, INTERNAL_TOOL_SEARCH_NAME);
            Ok(AnthropicConvertedItem::Message(AnthropicMessage {
                role: AnthropicRole::Assistant,
                content: vec![AnthropicContentBlock::ToolUse {
                    id: call_id,
                    name,
                    input: arguments,
                }],
            }))
        }
        ResponseItem::ToolSearchCall { execution, .. } => Err(ApiError::InvalidRequest {
            message: format!(
                "anthropic_messages supports replaying only client tool_search calls with call_id; got execution `{execution}`"
            ),
        }),
        ResponseItem::ToolSearchOutput {
            call_id: Some(call_id),
            status,
            execution,
            tools,
        } if execution == "client" => Ok(AnthropicConvertedItem::Message(AnthropicMessage {
            role: AnthropicRole::User,
            content: vec![AnthropicContentBlock::ToolResult {
                tool_use_id: call_id,
                content: anthropic_tool_search_result_content(status, execution, tools)?,
            }],
        })),
        ResponseItem::ToolSearchOutput { execution, .. } => Err(ApiError::InvalidRequest {
            message: format!(
                "anthropic_messages supports replaying only client tool_search outputs with call_id; got execution `{execution}`"
            ),
        }),
        ResponseItem::LocalShellCall { .. }
        | ResponseItem::CustomToolCall { .. }
        | ResponseItem::CustomToolCallOutput { .. }
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

fn anthropic_tool_input(arguments: &str) -> Result<Value, ApiError> {
    serde_json::from_str(arguments).map_err(|err| ApiError::InvalidRequest {
        message: format!("anthropic_messages function call arguments are not valid JSON: {err}"),
    })
}

fn anthropic_tool_result_content(output: FunctionCallOutputPayload) -> Result<String, ApiError> {
    match output.body {
        FunctionCallOutputBody::Text(content) => Ok(content),
        FunctionCallOutputBody::ContentItems(items) => {
            let mut text = Vec::new();
            for item in items {
                match item {
                    FunctionCallOutputContentItem::InputText { text: content } => {
                        text.push(content);
                    }
                    FunctionCallOutputContentItem::InputImage { .. }
                    | FunctionCallOutputContentItem::EncryptedContent { .. } => {
                        return Err(ApiError::InvalidRequest {
                            message: "anthropic_messages tool results support only text content"
                                .to_string(),
                        });
                    }
                }
            }
            Ok(text.join("\n"))
        }
    }
}

fn anthropic_tool_search_result_content(
    status: String,
    execution: String,
    tools: Vec<Value>,
) -> Result<String, ApiError> {
    serde_json::to_string(&serde_json::json!({
        "status": status,
        "execution": execution,
        "tools": tools,
    }))
    .map_err(|err| {
        ApiError::Stream(format!(
            "failed to encode anthropic_messages tool_search output: {err}"
        ))
    })
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
    fn converts_function_tool_history_to_anthropic_messages() {
        let converted = anthropic_messages_from_items(
            vec![
                ResponseItem::FunctionCall {
                    id: None,
                    name: "shell".to_string(),
                    namespace: None,
                    arguments: "{\"cmd\":\"ls\"}".to_string(),
                    call_id: "toolu_1".to_string(),
                },
                ResponseItem::FunctionCallOutput {
                    call_id: "toolu_1".to_string(),
                    output: FunctionCallOutputPayload::from_text("ok".to_string()),
                },
            ],
            &AnthropicToolNameMap::default(),
        )
        .expect("function tool history should convert");

        assert_eq!(
            converted,
            AnthropicConvertedMessages {
                messages: vec![
                    AnthropicMessage {
                        role: AnthropicRole::Assistant,
                        content: vec![AnthropicContentBlock::ToolUse {
                            id: "toolu_1".to_string(),
                            name: "shell".to_string(),
                            input: serde_json::json!({"cmd": "ls"}),
                        }],
                    },
                    AnthropicMessage {
                        role: AnthropicRole::User,
                        content: vec![AnthropicContentBlock::ToolResult {
                            tool_use_id: "toolu_1".to_string(),
                            content: "ok".to_string(),
                        }],
                    },
                ],
                system_messages: Vec::new(),
            }
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
    fn rejects_invalid_function_tool_arguments() {
        let err = anthropic_messages_from_items(
            vec![ResponseItem::FunctionCall {
                id: None,
                name: "shell".to_string(),
                namespace: None,
                arguments: "{".to_string(),
                call_id: "call-1".to_string(),
            }],
            &AnthropicToolNameMap::default(),
        )
        .expect_err("invalid arguments should be rejected");

        assert!(matches!(err, ApiError::InvalidRequest { .. }));
    }

    #[test]
    fn converts_namespaced_function_tool_history_to_anthropic_messages() {
        let mut tool_name_map = AnthropicToolNameMap::default();
        tool_name_map.register(
            Some("mcp__calendar__".to_string()),
            "create_event".to_string(),
        );

        let converted = anthropic_messages_from_items(
            vec![ResponseItem::FunctionCall {
                id: None,
                name: "create_event".to_string(),
                namespace: Some("mcp__calendar__".to_string()),
                arguments: "{}".to_string(),
                call_id: "call-1".to_string(),
            }],
            &tool_name_map,
        )
        .expect("namespaced function calls should convert");

        assert_eq!(
            converted,
            AnthropicConvertedMessages {
                messages: vec![AnthropicMessage {
                    role: AnthropicRole::Assistant,
                    content: vec![AnthropicContentBlock::ToolUse {
                        id: "call-1".to_string(),
                        name: "mcp__calendar__create_event".to_string(),
                        input: serde_json::json!({}),
                    }],
                }],
                system_messages: Vec::new(),
            }
        );
    }

    #[test]
    fn converts_function_tool_definitions_to_anthropic_tools() {
        let mut request = text_request();
        request.tools = vec![serde_json::json!({
            "type": "function",
            "name": "shell",
            "description": "Run a command.",
            "parameters": {
                "type": "object",
                "properties": {
                    "cmd": {"type": "string"}
                }
            }
        })];

        let body = anthropic_messages_body(request).expect("function tools should convert");

        assert_eq!(
            body["tools"],
            serde_json::json!([{
                "name": "shell",
                "description": "Run a command.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "cmd": {"type": "string"}
                    }
                }
            }])
        );
    }

    #[test]
    fn converts_namespace_tool_definitions_to_anthropic_tools() {
        let mut request = text_request();
        request.tools = vec![serde_json::json!({
            "type": "namespace",
            "name": "mcp__demo__",
            "description": "Demo namespace.",
            "tools": [{
                "type": "function",
                "name": "lookup_order",
                "description": "Lookup an order.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "order_id": {"type": "string"}
                    }
                }
            }]
        })];

        let body = anthropic_messages_body(request).expect("namespace tools should convert");

        assert_eq!(
            body["tools"],
            serde_json::json!([{
                "name": "mcp__demo__lookup_order",
                "description": "Demo namespace.\n\nLookup an order.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "order_id": {"type": "string"}
                    }
                }
            }])
        );
    }

    #[test]
    fn converts_tool_search_definition_to_anthropic_tool() {
        let mut request = text_request();
        request.tools = vec![serde_json::json!({
            "type": "tool_search",
            "execution": "deferred",
            "description": "Search tools.",
            "parameters": {"type": "object"}
        })];

        let body = anthropic_messages_body(request).expect("tool_search should convert");

        assert_eq!(
            body["tools"],
            serde_json::json!([{
                "name": "codex_tool_search",
                "description": "Search tools.",
                "input_schema": {"type": "object"}
            }])
        );
    }

    #[test]
    fn rejects_image_generation_tools_for_anthropic() {
        let mut request = text_request();
        request.tools = vec![serde_json::json!({
            "type": "image_generation",
            "output_format": "png"
        })];

        let err = anthropic_messages_body(request)
            .expect_err("Anthropic Messages should not expose image_generation");

        assert!(matches!(err, ApiError::InvalidRequest { .. }));
    }

    #[test]
    fn replays_tool_search_history_and_expands_returned_tools() {
        let mut request = text_request();
        request.tools = vec![serde_json::json!({
            "type": "tool_search",
            "execution": "deferred",
            "description": "Search tools.",
            "parameters": {"type": "object"}
        })];
        let returned_tools = vec![serde_json::json!({
            "type": "namespace",
            "name": "mcp__demo__",
            "description": "Demo namespace.",
            "tools": [{
                "type": "function",
                "name": "lookup_order",
                "description": "Lookup an order.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "order_id": {"type": "string"}
                    }
                }
            }]
        })];
        request.input = vec![
            text_message("user", "find the lookup tool"),
            ResponseItem::ToolSearchCall {
                id: Some("toolu_search".to_string()),
                call_id: Some("toolu_search".to_string()),
                status: None,
                execution: "client".to_string(),
                arguments: serde_json::json!({"query": "lookup order", "limit": 1}),
            },
            ResponseItem::ToolSearchOutput {
                call_id: Some("toolu_search".to_string()),
                status: "completed".to_string(),
                execution: "client".to_string(),
                tools: returned_tools.clone(),
            },
        ];

        let body = anthropic_messages_body(request).expect("tool_search history should convert");

        assert_eq!(
            body["tools"],
            serde_json::json!([
                {
                    "name": "codex_tool_search",
                    "description": "Search tools.",
                    "input_schema": {"type": "object"}
                },
                {
                    "name": "mcp__demo__lookup_order",
                    "description": "Demo namespace.\n\nLookup an order.",
                    "input_schema": {
                        "type": "object",
                        "properties": {
                            "order_id": {"type": "string"}
                        }
                    }
                }
            ])
        );
        assert_eq!(
            body["messages"][1],
            serde_json::json!({
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_search",
                    "name": "codex_tool_search",
                    "input": {"query": "lookup order", "limit": 1}
                }]
            })
        );
        let tool_search_result = body["messages"][2]["content"][0]["content"]
            .as_str()
            .expect("tool_search output should be textual JSON");
        let tool_search_result: serde_json::Value =
            serde_json::from_str(tool_search_result).expect("tool_search result should be JSON");
        assert_eq!(
            tool_search_result,
            serde_json::json!({
                "status": "completed",
                "execution": "client",
                "tools": returned_tools,
            })
        );
    }

    #[test]
    fn rejects_empty_message_content() {
        let mut request = text_request();
        request.input = vec![ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: Vec::new(),
            phase: None,
        }];

        let err = anthropic_messages_body(request).expect_err("empty content should be rejected");

        assert!(matches!(err, ApiError::InvalidRequest { .. }));
    }
}
