#![cfg(not(target_os = "windows"))]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use anyhow::Context;
use core_test_support::test_codex_exec::test_codex_exec;
use pretty_assertions::assert_eq;
use serde_json::Value;
use wiremock::Mock;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn anthropic_messages_provider_smoke_uses_messages_api() -> anyhow::Result<()> {
    let test = test_codex_exec();
    let server = wiremock::MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(anthropic_text_sse("msg-exec", "Anthropic exec done")),
        )
        .expect(1)
        .mount(&server)
        .await;

    create_anthropic_config_toml(test.home_path(), &server.uri())?;

    test.cmd()
        .env("ANTHROPIC_API_KEY", "anthropic-test-key")
        .arg("--skip-git-repo-check")
        .arg("say hello with anthropic")
        .assert()
        .success();

    let requests = server
        .received_requests()
        .await
        .context("failed to fetch received requests")?;
    let messages_requests = requests
        .iter()
        .filter(|request| request.url.path() == "/v1/messages")
        .collect::<Vec<_>>();
    assert_eq!(messages_requests.len(), 1);

    let request = messages_requests[0];
    assert_eq!(
        request
            .headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok()),
        Some("anthropic-test-key")
    );
    assert!(request.headers.get("authorization").is_none());
    assert_eq!(
        request
            .headers
            .get("anthropic-version")
            .and_then(|value| value.to_str().ok()),
        Some("2023-06-01")
    );

    let body = request
        .body_json::<Value>()
        .context("request body should be JSON")?;
    assert_eq!(
        body.get("model").and_then(Value::as_str),
        Some("claude-sonnet-4-6")
    );
    assert_eq!(body.get("stream").and_then(Value::as_bool), Some(true));
    assert_eq!(body.get("input"), None);
    assert!(
        body.get("messages")
            .and_then(Value::as_array)
            .is_some_and(|messages| !messages.is_empty()),
        "Anthropic request should send Messages API history: {body:?}"
    );

    Ok(())
}

fn create_anthropic_config_toml(
    codex_home: &std::path::Path,
    server_uri: &str,
) -> std::io::Result<()> {
    std::fs::write(
        codex_home.join("config.toml"),
        format!(
            r#"
model = "claude-sonnet-4-6"
approval_policy = "never"
sandbox_mode = "read-only"

model_provider = "anthropic_test"

[model_providers.anthropic_test]
name = "Anthropic Test"
base_url = "{server_uri}/v1"
env_key = "ANTHROPIC_API_KEY"
wire_api = "anthropic_messages"
request_max_retries = 0
stream_max_retries = 0

[model_providers.anthropic_test.http_headers]
anthropic-version = "2023-06-01"
"#
        ),
    )
}

fn anthropic_text_sse(message_id: &str, text: &str) -> String {
    format!(
        concat!(
            "event: message_start\n",
            "data: {{\"type\":\"message_start\",\"message\":{{\"id\":\"{message_id}\",\"usage\":{{\"input_tokens\":1,\"output_tokens\":0}}}}}}\n\n",
            "event: content_block_start\n",
            "data: {{\"type\":\"content_block_start\",\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\n",
            "event: content_block_delta\n",
            "data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":{text_json}}}}}\n\n",
            "event: content_block_stop\n",
            "data: {{\"type\":\"content_block_stop\",\"index\":0}}\n\n",
            "event: message_delta\n",
            "data: {{\"type\":\"message_delta\",\"usage\":{{\"output_tokens\":1}}}}\n\n",
            "event: message_stop\n",
            "data: {{\"type\":\"message_stop\"}}\n\n",
        ),
        message_id = message_id,
        text_json = serde_json::json!(text),
    )
}
