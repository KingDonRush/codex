mod event;

use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::telemetry::SseTelemetry;
use codex_client::ByteStream;
use codex_client::StreamResponse;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::TokenUsage;
use event::AnthropicContentBlock;
use event::AnthropicDelta;
use event::AnthropicError;
use event::AnthropicMessageStart;
use event::AnthropicStreamEvent;
use event::AnthropicUsageDelta;
use eventsource_stream::Eventsource;
use futures::StreamExt;
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

pub fn spawn_anthropic_messages_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    _turn_state: Option<Arc<OnceLock<String>>>,
) -> ResponseStream {
    let upstream_request_id = stream_response
        .headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(async move {
        process_sse(stream_response.bytes, tx_event, idle_timeout, telemetry).await;
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

    fn append_text_delta(&mut self, index: usize, text: String) -> ResponseEvent {
        self.text_blocks.entry(index).or_default().push_str(&text);
        ResponseEvent::OutputTextDelta(text)
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
                Some(_) | None => Ok(None),
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
                Some(_) | None => Ok(None),
            }
        }
        "content_block_stop" => {
            let Some(index) = event.index else {
                return Err(ApiError::Stream(
                    "Anthropic content_block_stop missing index".to_string(),
                ));
            };
            Ok(state.stop_text_block(index))
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
) {
    let mut stream = stream.eventsource();
    let mut state = AnthropicStreamState::default();

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
    async fn maps_anthropic_error() {
        let body = concat!(
            "event: error\n",
            "data: {\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"bad request\"}}\n\n",
        );

        let events = collect_events(&[body.as_bytes()]).await;

        assert!(matches!(
            &events[0],
            Err(ApiError::InvalidRequest { message }) if message == "bad request"
        ));
    }
}
