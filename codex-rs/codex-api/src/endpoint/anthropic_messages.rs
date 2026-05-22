mod request;

use crate::auth::SharedAuthProvider;
use crate::common::ResponseStream;
use crate::common::ResponsesApiRequest;
use crate::endpoint::anthropic_messages::request::anthropic_messages_request;
use crate::endpoint::responses::ResponsesOptions;
use crate::endpoint::session::EndpointSession;
use crate::error::ApiError;
use crate::provider::Provider;
use crate::requests::Compression;
use crate::sse::spawn_anthropic_messages_stream;
use crate::telemetry::SseTelemetry;
use codex_client::HttpTransport;
use codex_client::RequestCompression;
use codex_client::RequestTelemetry;
use http::HeaderMap;
use http::HeaderValue;
use http::Method;
use serde_json::Value;
use std::sync::Arc;
use std::sync::OnceLock;
use tracing::instrument;

pub struct AnthropicMessagesClient<T: HttpTransport> {
    session: EndpointSession<T>,
    sse_telemetry: Option<Arc<dyn SseTelemetry>>,
}

impl<T: HttpTransport> AnthropicMessagesClient<T> {
    pub fn new(transport: T, provider: Provider, auth: SharedAuthProvider) -> Self {
        Self {
            session: EndpointSession::new(transport, provider, auth),
            sse_telemetry: None,
        }
    }

    pub fn with_telemetry(
        self,
        request: Option<Arc<dyn RequestTelemetry>>,
        sse: Option<Arc<dyn SseTelemetry>>,
    ) -> Self {
        Self {
            session: self.session.with_request_telemetry(request),
            sse_telemetry: sse,
        }
    }

    #[instrument(
        name = "anthropic_messages.stream_request",
        level = "info",
        skip_all,
        fields(
            transport = "anthropic_messages_http",
            http.method = "POST",
            api.path = "messages"
        )
    )]
    pub async fn stream_request(
        &self,
        request: ResponsesApiRequest,
        options: ResponsesOptions,
    ) -> Result<ResponseStream, ApiError> {
        let ResponsesOptions {
            extra_headers,
            compression,
            turn_state,
            ..
        } = options;
        let body = anthropic_messages_request(request)?;

        self.stream(body, extra_headers, compression, turn_state)
            .await
    }

    fn path() -> &'static str {
        "messages"
    }

    #[instrument(
        name = "anthropic_messages.stream",
        level = "info",
        skip_all,
        fields(
            transport = "anthropic_messages_http",
            http.method = "POST",
            api.path = "messages",
            turn.has_state = turn_state.is_some()
        )
    )]
    pub async fn stream(
        &self,
        body: Value,
        extra_headers: HeaderMap,
        compression: Compression,
        turn_state: Option<Arc<OnceLock<String>>>,
    ) -> Result<ResponseStream, ApiError> {
        let request_compression = match compression {
            Compression::None => RequestCompression::None,
            Compression::Zstd => RequestCompression::Zstd,
        };

        let stream_response = self
            .session
            .stream_with(
                Method::POST,
                Self::path(),
                extra_headers,
                Some(body),
                |req| {
                    req.headers.insert(
                        http::header::ACCEPT,
                        HeaderValue::from_static("text/event-stream"),
                    );
                    req.compression = request_compression;
                },
            )
            .await?;

        Ok(spawn_anthropic_messages_stream(
            stream_response,
            self.session.provider().stream_idle_timeout,
            self.sse_telemetry.clone(),
            turn_state,
        ))
    }
}
