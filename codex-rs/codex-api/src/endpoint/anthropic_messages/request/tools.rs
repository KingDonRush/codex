use super::ANTHROPIC_TOOL_SEARCH_NAME;
use super::AnthropicCustomTool;
use super::AnthropicTool;
use super::AnthropicWebSearchTool;
use super::INTERNAL_TOOL_SEARCH_NAME;
use crate::anthropic_tool_names::AnthropicToolNameMap;
use crate::error::ApiError;
use codex_protocol::models::ResponseItem;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct ResponsesFunctionTool {
    name: String,
    #[serde(default)]
    description: String,
    parameters: Value,
}

#[derive(Debug, Deserialize)]
struct ResponsesNamespace {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    tools: Vec<ResponsesNamespaceTool>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ResponsesNamespaceTool {
    #[serde(rename = "function")]
    Function(ResponsesFunctionTool),
}

#[derive(Debug, Deserialize)]
struct ResponsesToolSearchTool {
    #[serde(default)]
    description: String,
    parameters: Value,
}

#[derive(Debug, Deserialize)]
struct ResponsesWebSearchTool {
    #[serde(default)]
    external_web_access: Option<bool>,
    #[serde(default)]
    filters: Option<ResponsesWebSearchFilters>,
    #[serde(default)]
    user_location: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ResponsesWebSearchFilters {
    #[serde(default)]
    allowed_domains: Option<Vec<String>>,
}

#[derive(Default)]
pub(super) struct AnthropicConvertedTools {
    pub(super) tools: Vec<AnthropicTool>,
    pub(super) tool_name_map: AnthropicToolNameMap,
}

pub(super) fn anthropic_tools_from_responses_api_tools(
    tools: Vec<Value>,
) -> Result<AnthropicConvertedTools, ApiError> {
    let mut converted = AnthropicConvertedTools::default();
    for tool in tools {
        anthropic_tools_from_responses_api_tool(tool, &mut converted)?;
    }
    Ok(converted)
}

pub(super) fn anthropic_tools_from_tool_search_outputs(
    items: &[ResponseItem],
    converted: &mut AnthropicConvertedTools,
) -> Result<(), ApiError> {
    for item in items {
        let ResponseItem::ToolSearchOutput {
            execution, tools, ..
        } = item
        else {
            continue;
        };

        if execution != "client" {
            continue;
        }

        for tool in tools {
            anthropic_tools_from_responses_api_tool(tool.clone(), converted)?;
        }
    }

    Ok(())
}

fn anthropic_tools_from_responses_api_tool(
    tool: Value,
    converted: &mut AnthropicConvertedTools,
) -> Result<(), ApiError> {
    let tool_type = tool
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("<missing>");
    match tool_type {
        "function" => {
            let function_tool =
                serde_json::from_value::<ResponsesFunctionTool>(tool).map_err(|err| {
                    ApiError::InvalidRequest {
                        message: format!("invalid function tool for anthropic_messages: {err}"),
                    }
                })?;
            let name = converted.tool_name_map.register(None, function_tool.name);
            converted
                .tools
                .push(AnthropicTool::Custom(AnthropicCustomTool {
                    name,
                    description: function_tool.description,
                    input_schema: function_tool.parameters,
                }));
            Ok(())
        }
        "namespace" => {
            let namespace = serde_json::from_value::<ResponsesNamespace>(tool).map_err(|err| {
                ApiError::InvalidRequest {
                    message: format!("invalid namespace tool for anthropic_messages: {err}"),
                }
            })?;
            for namespace_tool in namespace.tools {
                match namespace_tool {
                    ResponsesNamespaceTool::Function(function_tool) => {
                        let name = converted
                            .tool_name_map
                            .register(Some(namespace.name.clone()), function_tool.name);
                        let description = match (
                            namespace.description.is_empty(),
                            function_tool.description.is_empty(),
                        ) {
                            (true, true) => String::new(),
                            (true, false) => function_tool.description,
                            (false, true) => namespace.description.clone(),
                            (false, false) => {
                                format!(
                                    "{}\n\n{}",
                                    namespace.description, function_tool.description
                                )
                            }
                        };
                        converted
                            .tools
                            .push(AnthropicTool::Custom(AnthropicCustomTool {
                                name,
                                description,
                                input_schema: function_tool.parameters,
                            }));
                    }
                }
            }
            Ok(())
        }
        "tool_search" => {
            let tool_search =
                serde_json::from_value::<ResponsesToolSearchTool>(tool).map_err(|err| {
                    ApiError::InvalidRequest {
                        message: format!("invalid tool_search tool for anthropic_messages: {err}"),
                    }
                })?;
            let name = converted.tool_name_map.register_with_anthropic_name(
                None,
                INTERNAL_TOOL_SEARCH_NAME.to_string(),
                ANTHROPIC_TOOL_SEARCH_NAME,
            );
            converted
                .tools
                .push(AnthropicTool::Custom(AnthropicCustomTool {
                    name,
                    description: tool_search.description,
                    input_schema: tool_search.parameters,
                }));
            Ok(())
        }
        "web_search" => {
            let web_search =
                serde_json::from_value::<ResponsesWebSearchTool>(tool).map_err(|err| {
                    ApiError::InvalidRequest {
                        message: format!("invalid web_search tool for anthropic_messages: {err}"),
                    }
                })?;
            if web_search.external_web_access == Some(false) {
                return Ok(());
            }
            converted
                .tools
                .push(AnthropicTool::WebSearch(AnthropicWebSearchTool {
                    kind: "web_search_20250305".to_string(),
                    name: "web_search".to_string(),
                    allowed_domains: web_search
                        .filters
                        .and_then(|filters| filters.allowed_domains),
                    user_location: web_search.user_location,
                }));
            Ok(())
        }
        _ => Err(ApiError::InvalidRequest {
            message: format!(
                "anthropic_messages supports only function, namespace, tool_search, and web_search tools in this phase; got `{tool_type}`"
            ),
        }),
    }
}
