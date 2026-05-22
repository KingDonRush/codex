mod event;

use crate::anthropic_tool_names::AnthropicToolNameMap;
use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::telemetry::SseTelemetry;
use codex_client::ByteStream;
use codex_client::StreamResponse;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::WebSearchAction;
use codex_protocol::protocol::TokenUsage;
use event::AnthropicContentBlock;
use event::AnthropicDelta;
use event::AnthropicError;
use event::AnthropicMessageStart;
use event::AnthropicStreamEvent;
use event::AnthropicUsageDelta;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::debug;
use tracing::trace;

const REQUEST_ID_HEADER: &str = "request-id";
const INTERNAL_TOOL_SEARCH_NAME: &str = "tool_search";

pub fn spawn_anthropic_messages_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    _turn_state: Option<Arc<OnceLock<String>>>,
    tool_name_map: AnthropicToolNameMap,
) -> ResponseStream {
    let upstream_request_id = stream_response
        .headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(async move {
        process_sse(
            stream_response.bytes,
            tx_event,
            idle_timeout,
            telemetry,
            tool_name_map,
        )
        .await;
    });

    ResponseStream {
        rx_event,
        upstream_request_id,
    }
}

#[derive(Default)]
struct AnthropicStreamState {
    message_id: Option<String>,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    text_blocks: HashMap<usize, String>,
    tool_blocks: HashMap<usize, AnthropicToolUseBlock>,
    server_tool_blocks: HashMap<usize, AnthropicToolUseBlock>,
    pending_web_searches: HashMap<String, WebSearchAction>,
    tool_name_map: AnthropicToolNameMap,
}

struct AnthropicToolUseBlock {
    id: String,
    name: String,
    initial_input: Value,
    input_json_delta: String,
}

impl AnthropicStreamState {
    fn start_message(&mut self, message: AnthropicMessageStart) -> ResponseEvent {
        self.message_id = Some(message.id);
        if let Some(usage) = message.usage {
            self.input_tokens = Some(usage.input_tokens);
            self.output_tokens = Some(usage.output_tokens);
        }
        ResponseEvent::Created
    }

    fn start_text_block(&mut self, index: usize, text: String) -> ResponseEvent {
        self.text_blocks.insert(index, text.clone());
        ResponseEvent::OutputItemAdded(ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText { text }],
            phase: None,
        })
    }

    fn start_tool_use_block(&mut self, index: usize, id: String, name: String, input: Value) {
        self.tool_blocks.insert(
            index,
            AnthropicToolUseBlock {
                id,
                name,
                initial_input: input,
                input_json_delta: String::new(),
            },
        );
    }

    fn start_server_tool_use_block(
        &mut self,
        index: usize,
        id: String,
        name: String,
        input: Value,
    ) {
        self.server_tool_blocks.insert(
            index,
            AnthropicToolUseBlock {
                id,
                name,
                initial_input: input,
                input_json_delta: String::new(),
            },
        );
    }

    fn start_web_search_tool_result(
        &mut self,
        tool_use_id: String,
        content: Value,
    ) -> ResponseEvent {
        let action = self.pending_web_searches.remove(&tool_use_id);
        let results = (!content.is_null()).then_some(content);
        ResponseEvent::OutputItemDone(ResponseItem::WebSearchCall {
            id: Some(tool_use_id),
            status: Some("completed".to_string()),
            action,
            results,
        })
    }

    fn append_text_delta(&mut self, index: usize, text: String) -> ResponseEvent {
        self.text_blocks.entry(index).or_default().push_str(&text);
        ResponseEvent::OutputTextDelta(text)
    }

    fn append_tool_input_delta(
        &mut self,
        index: usize,
        partial_json: String,
    ) -> Result<(), ApiError> {
        let Some(block) = self.tool_blocks.get_mut(&index) else {
            if let Some(block) = self.server_tool_blocks.get_mut(&index) {
                block.input_json_delta.push_str(&partial_json);
                return Ok(());
            }

            return Err(ApiError::Stream(format!(
                "Anthropic input_json_delta received before tool_use start for block {index}",
            )));
        };

        block.input_json_delta.push_str(&partial_json);
        Ok(())
    }

    fn stop_text_block(&mut self, index: usize) -> Option<ResponseEvent> {
        let text = self.text_blocks.remove(&index)?;
        Some(ResponseEvent::OutputItemDone(ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText { text }],
            phase: None,
        }))
    }

    fn stop_tool_use_block(&mut self, index: usize) -> Result<Option<ResponseEvent>, ApiError> {
        let Some(block) = self.tool_blocks.remove(&index) else {
            return Ok(None);
        };

        let arguments = block.arguments()?;
        let tool_name = self.tool_name_map.resolve(&block.name).ok_or_else(|| {
            ApiError::Stream(format!(
                "Anthropic tool_use referenced unknown tool `{}`",
                block.name
            ))
        })?;
        if tool_name.namespace.is_none() && tool_name.name == INTERNAL_TOOL_SEARCH_NAME {
            return Ok(Some(ResponseEvent::OutputItemDone(
                ResponseItem::ToolSearchCall {
                    id: Some(block.id.clone()),
                    call_id: Some(block.id),
                    status: None,
                    execution: "client".to_string(),
                    arguments: serde_json::from_str(&arguments).map_err(|err| {
                        ApiError::Stream(format!(
                            "Anthropic tool_search input is not valid JSON: {err}"
                        ))
                    })?,
                },
            )));
        }
        Ok(Some(ResponseEvent::OutputItemDone(
            ResponseItem::FunctionCall {
                id: Some(block.id.clone()),
                name: tool_name.name,
                namespace: tool_name.namespace,
                arguments,
                call_id: block.id,
            },
        )))
    }

    fn stop_server_tool_use_block(
        &mut self,
        index: usize,
    ) -> Result<Option<ResponseEvent>, ApiError> {
        let Some(block) = self.server_tool_blocks.remove(&index) else {
            return Ok(None);
        };

        if block.name == "web_search" {
            let action = block.web_search_action()?;
            self.pending_web_searches.insert(block.id, action);
        }

        Ok(None)
    }

    fn stop_content_block(&mut self, index: usize) -> Result<Option<ResponseEvent>, ApiError> {
        if let Some(event) = self.stop_text_block(index) {
            return Ok(Some(event));
        }

        if let Some(event) = self.stop_tool_use_block(index)? {
            return Ok(Some(event));
        }

        self.stop_server_tool_use_block(index)
    }

    fn update_usage(&mut self, usage: Option<AnthropicUsageDelta>) {
        if let Some(usage) = usage {
            self.output_tokens = Some(usage.output_tokens);
        }
    }

    fn complete(&self) -> ResponseEvent {
        let input_tokens = self.input_tokens.unwrap_or(0);
        let output_tokens = self.output_tokens.unwrap_or(0);
        let token_usage =
            (self.input_tokens.is_some() || self.output_tokens.is_some()).then(|| TokenUsage {
                input_tokens,
                cached_input_tokens: 0,
                output_tokens,
                reasoning_output_tokens: 0,
                total_tokens: input_tokens + output_tokens,
            });

        ResponseEvent::Completed {
            response_id: self
                .message_id
                .clone()
                .unwrap_or_else(|| "anthropic-message".to_string()),
            token_usage,
            end_turn: Some(true),
        }
    }
}

impl AnthropicToolUseBlock {
    fn arguments(&self) -> Result<String, ApiError> {
        let arguments = if self.input_json_delta.is_empty() {
            serde_json::to_string(&self.initial_input).map_err(|err| {
                ApiError::Stream(format!("failed to encode Anthropic tool_use input: {err}"))
            })?
        } else {
            self.input_json_delta.clone()
        };

        serde_json::from_str::<Value>(&arguments).map_err(|err| {
            ApiError::Stream(format!("Anthropic tool_use input is not valid JSON: {err}"))
        })?;

        Ok(arguments)
    }

    fn arguments_value(&self, context: &str) -> Result<Value, ApiError> {
        let arguments = self.arguments()?;
        serde_json::from_str::<Value>(&arguments).map_err(|err| {
            ApiError::Stream(format!(
                "Anthropic {context} input is not valid JSON: {err}"
            ))
        })
    }

    fn web_search_action(&self) -> Result<WebSearchAction, ApiError> {
        let arguments = self.arguments_value("web_search")?;
        Ok(WebSearchAction::Search {
            query: arguments
                .get("query")
                .and_then(Value::as_str)
                .map(str::to_string),
            queries: None,
        })
    }
}

fn process_anthropic_event(
    state: &mut AnthropicStreamState,
    event: AnthropicStreamEvent,
) -> Result<Option<ResponseEvent>, ApiError> {
    match event.kind.as_str() {
        "message_start" => {
            let message = event.message.ok_or_else(|| {
                ApiError::Stream("Anthropic message_start missing message".to_string())
            })?;
            Ok(Some(state.start_message(message)))
        }
        "content_block_start" => {
            let Some(index) = event.index else {
                return Err(ApiError::Stream(
                    "Anthropic content_block_start missing index".to_string(),
                ));
            };
            match event.content_block {
                Some(AnthropicContentBlock::Text { text }) => {
                    Ok(Some(state.start_text_block(index, text)))
                }
                Some(AnthropicContentBlock::ToolUse { id, name, input }) => {
                    state.start_tool_use_block(index, id, name, input);
                    Ok(None)
                }
                Some(AnthropicContentBlock::ServerToolUse { id, name, input }) => {
                    state.start_server_tool_use_block(index, id, name, input);
                    Ok(None)
                }
                Some(AnthropicContentBlock::WebSearchToolResult {
                    tool_use_id,
                    content,
                }) => Ok(Some(
                    state.start_web_search_tool_result(tool_use_id, content),
                )),
                Some(AnthropicContentBlock::Other) | None => Ok(None),
            }
        }
        "content_block_delta" => {
            let Some(index) = event.index else {
                return Err(ApiError::Stream(
                    "Anthropic content_block_delta missing index".to_string(),
                ));
            };
            match event.delta.and_then(|delta| {
                serde_json::from_value::<AnthropicDelta>(delta)
                    .inspect_err(|err| debug!("failed to parse Anthropic content delta: {err}"))
                    .ok()
            }) {
                Some(AnthropicDelta::TextDelta { text }) => {
                    Ok(Some(state.append_text_delta(index, text)))
                }
                Some(AnthropicDelta::InputJsonDelta { partial_json }) => {
                    state.append_tool_input_delta(index, partial_json)?;
                    Ok(None)
                }
                Some(
                    AnthropicDelta::ThinkingDelta {}
                    | AnthropicDelta::SignatureDelta {}
                    | AnthropicDelta::Other,
                )
                | None => Ok(None),
            }
        }
        "content_block_stop" => {
            let Some(index) = event.index else {
                return Err(ApiError::Stream(
                    "Anthropic content_block_stop missing index".to_string(),
                ));
            };
            state.stop_content_block(index)
        }
        "message_delta" => {
            state.update_usage(event.usage);
            Ok(None)
        }
        "message_stop" => Ok(Some(state.complete())),
        "ping" => Ok(None),
        "error" => Err(map_anthropic_error(event.error)),
        _ => {
            trace!("unhandled Anthropic event: {}", event.kind);
            Ok(None)
        }
    }
}

pub async fn process_sse(
    stream: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    tool_name_map: AnthropicToolNameMap,
) {
    let mut stream = stream.eventsource();
    let mut state = AnthropicStreamState {
        tool_name_map,
        ..Default::default()
    };

    loop {
        let start = Instant::now();
        let response = timeout(idle_timeout, stream.next()).await;
        if let Some(t) = telemetry.as_ref() {
            t.on_sse_poll(&response, start.elapsed());
        }
        let sse = match response {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(err))) => {
                debug!("Anthropic SSE error: {err:#}");
                let _ = tx_event.send(Err(ApiError::Stream(err.to_string()))).await;
                return;
            }
            Ok(None) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream(
                        "stream closed before message_stop".to_string(),
                    )))
                    .await;
                return;
            }
            Err(_) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream("idle timeout waiting for SSE".into())))
                    .await;
                return;
            }
        };

        trace!("Anthropic SSE event: {}", &sse.data);

        let event: AnthropicStreamEvent = match serde_json::from_str(&sse.data) {
            Ok(event) => event,
            Err(err) => {
                debug!(
                    "failed to parse Anthropic SSE event: {err}, data: {}",
                    &sse.data
                );
                continue;
            }
        };

        match process_anthropic_event(&mut state, event) {
            Ok(Some(event)) => {
                let is_completed = matches!(event, ResponseEvent::Completed { .. });
                if tx_event.send(Ok(event)).await.is_err() {
                    return;
                }
                if is_completed {
                    return;
                }
            }
            Ok(None) => {}
            Err(err) => {
                let _ = tx_event.send(Err(err)).await;
                return;
            }
        }
    }
}

fn map_anthropic_error(error: Option<AnthropicError>) -> ApiError {
    let Some(error) = error else {
        return ApiError::Stream("Anthropic stream error".to_string());
    };

    match error.kind.as_str() {
        "invalid_request_error" => ApiError::InvalidRequest {
            message: error.message,
        },
        "overloaded_error" => ApiError::ServerOverloaded,
        _ => ApiError::Stream(error.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use codex_client::TransportError;
    use futures::TryStreamExt;
    use pretty_assertions::assert_eq;
    use tokio_test::io::Builder as IoBuilder;
    use tokio_util::io::ReaderStream;

    async fn collect_events(chunks: &[&[u8]]) -> Vec<Result<ResponseEvent, ApiError>> {
        collect_events_with_tool_name_map(chunks, AnthropicToolNameMap::default()).await
    }

    async fn collect_events_with_tool_name_map(
        chunks: &[&[u8]],
        tool_name_map: AnthropicToolNameMap,
    ) -> Vec<Result<ResponseEvent, ApiError>> {
        let mut builder = IoBuilder::new();
        for chunk in chunks {
            builder.read(chunk);
        }

        let reader = builder.build();
        let stream =
            ReaderStream::new(reader).map_err(|err| TransportError::Network(err.to_string()));
        let (tx, mut rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(16);
        tokio::spawn(process_sse(
            Box::pin(stream),
            tx,
            Duration::from_millis(1000),
            /*telemetry*/ None,
            tool_name_map,
        ));

        let mut events = Vec::new();
        while let Some(ev) = rx.recv().await {
            events.push(ev);
        }
        events
    }

    #[tokio::test]
    async fn parses_text_stream() {
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"lo\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":3}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );

        let events = collect_events(&[body.as_bytes()]).await;

        assert_eq!(events.len(), 6);
        assert_matches!(events[0], Ok(ResponseEvent::Created));
        assert_matches!(events[1], Ok(ResponseEvent::OutputItemAdded(_)));
        assert_matches!(
            &events[2],
            Ok(ResponseEvent::OutputTextDelta(delta)) if delta == "Hel"
        );
        assert_matches!(
            &events[3],
            Ok(ResponseEvent::OutputTextDelta(delta)) if delta == "lo"
        );
        assert_matches!(
            &events[4],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. }))
                if content == &vec![ContentItem::OutputText { text: "Hello".to_string() }]
        );
        assert_matches!(
            &events[5],
            Ok(ResponseEvent::Completed {
                response_id,
                token_usage: Some(TokenUsage { input_tokens: 10, output_tokens: 3, .. }),
                end_turn: Some(true)
            }) if response_id == "msg_1"
        );
    }

    #[tokio::test]
    async fn ignores_non_text_block_stop() {
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"internal\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );

        let events = collect_events(&[body.as_bytes()]).await;

        assert_eq!(events.len(), 2);
        assert_matches!(events[0], Ok(ResponseEvent::Created));
        assert_matches!(events[1], Ok(ResponseEvent::Completed { .. }));
    }

    #[tokio::test]
    async fn parses_tool_use_stream() {
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"shell\",\"input\":{}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"cmd\\\":\\\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"ls\\\"}\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );

        let events = collect_events(&[body.as_bytes()]).await;

        assert_eq!(events.len(), 3);
        assert_matches!(events[0], Ok(ResponseEvent::Created));
        assert_matches!(
            &events[1],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                id: Some(id),
                name,
                namespace: None,
                arguments,
                call_id,
            })) if id == "toolu_1"
                && name == "shell"
                && arguments == "{\"cmd\":\"ls\"}"
                && call_id == "toolu_1"
        );
        assert_matches!(events[2], Ok(ResponseEvent::Completed { .. }));
    }

    #[tokio::test]
    async fn parses_namespaced_tool_use_stream() {
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"mcp__demo__lookup_order\",\"input\":{\"order_id\":\"ord_1\"}}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let mut tool_name_map = AnthropicToolNameMap::default();
        tool_name_map.register(Some("mcp__demo__".to_string()), "lookup_order".to_string());

        let events = collect_events_with_tool_name_map(&[body.as_bytes()], tool_name_map).await;

        assert_eq!(events.len(), 3);
        assert_matches!(events[0], Ok(ResponseEvent::Created));
        assert_matches!(
            &events[1],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                name,
                namespace: Some(namespace),
                arguments,
                ..
            })) if name == "lookup_order"
                && namespace == "mcp__demo__"
                && arguments == "{\"order_id\":\"ord_1\"}"
        );
        assert_matches!(events[2], Ok(ResponseEvent::Completed { .. }));
    }

    #[tokio::test]
    async fn parses_tool_search_tool_use_stream() {
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_search\",\"name\":\"codex_tool_search\",\"input\":{\"query\":\"calendar\",\"limit\":1}}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let mut tool_name_map = AnthropicToolNameMap::default();
        tool_name_map.register_with_anthropic_name(
            None,
            "tool_search".to_string(),
            "codex_tool_search",
        );

        let events = collect_events_with_tool_name_map(&[body.as_bytes()], tool_name_map).await;

        assert_eq!(events.len(), 3);
        assert_matches!(events[0], Ok(ResponseEvent::Created));
        assert_matches!(
            &events[1],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::ToolSearchCall {
                call_id: Some(call_id),
                execution,
                arguments,
                ..
            })) if call_id == "toolu_search"
                && execution == "client"
                && arguments == &serde_json::json!({"query": "calendar", "limit": 1})
        );
        assert_matches!(events[2], Ok(ResponseEvent::Completed { .. }));
    }

    #[tokio::test]
    async fn parses_web_search_server_tool_stream() {
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvtoolu_1\",\"name\":\"web_search\",\"input\":{}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"query\\\":\\\"\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"latest codex news\\\"}\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"web_search_tool_result\",\"tool_use_id\":\"srvtoolu_1\",\"content\":[{\"type\":\"web_search_result\",\"title\":\"Codex\",\"url\":\"https://example.com\"}]}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":3,\"content_block\":{\"type\":\"text\",\"text\":\"Found it\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":3}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );

        let events = collect_events(&[body.as_bytes()]).await;

        assert_eq!(events.len(), 5);
        assert_matches!(events[0], Ok(ResponseEvent::Created));
        assert_matches!(
            &events[1],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::WebSearchCall {
                id: Some(id),
                status: Some(status),
                action: Some(WebSearchAction::Search { query: Some(query), queries: None }),
                results: Some(results),
            })) if id == "srvtoolu_1"
                && status == "completed"
                && query == "latest codex news"
                && results == &serde_json::json!([{
                    "type": "web_search_result",
                    "title": "Codex",
                    "url": "https://example.com"
                }])
        );
        assert_matches!(events[2], Ok(ResponseEvent::OutputItemAdded(_)));
        assert_matches!(
            &events[3],
            Ok(ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. }))
                if content == &vec![ContentItem::OutputText { text: "Found it".to_string() }]
        );
        assert_matches!(events[4], Ok(ResponseEvent::Completed { .. }));
    }

    #[tokio::test]
    async fn errors_on_unknown_mapped_tool_use_name() {
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"unknown_tool\",\"input\":{}}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        );
        let mut tool_name_map = AnthropicToolNameMap::default();
        tool_name_map.register(None, "known_tool".to_string());

        let events = collect_events_with_tool_name_map(&[body.as_bytes()], tool_name_map).await;

        assert_eq!(events.len(), 2);
        assert_matches!(events[0], Ok(ResponseEvent::Created));
        assert_matches!(
            &events[1],
            Err(ApiError::Stream(message)) if message.contains("unknown tool")
        );
    }

    #[tokio::test]
    async fn errors_on_invalid_tool_use_json() {
        let body = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"shell\",\"input\":{}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"cmd\\\"\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        );

        let events = collect_events(&[body.as_bytes()]).await;

        assert_eq!(events.len(), 2);
        assert_matches!(events[0], Ok(ResponseEvent::Created));
        assert_matches!(
            &events[1],
            Err(ApiError::Stream(message)) if message.contains("tool_use input is not valid JSON")
        );
    }

    #[tokio::test]
    async fn maps_stream_error() {
        let body = concat!(
            "event: error\n",
            "data: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n",
        );

        let events = collect_events(&[body.as_bytes()]).await;

        assert_eq!(events.len(), 1);
        assert_matches!(events[0], Err(ApiError::ServerOverloaded));
    }
}
