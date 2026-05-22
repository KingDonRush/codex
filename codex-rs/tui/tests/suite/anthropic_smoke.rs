use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;
use std::thread::sleep;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use anyhow::Result;
use pretty_assertions::assert_eq;
use tempfile::tempdir;
use wiremock::Mock;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires tmux and a locally built codex binary; run with --ignored for manual Anthropic TUI smoke"]
async fn tmux_anthropic_messages_provider_renders_response() -> Result<()> {
    if cfg!(windows) {
        return Ok(());
    }
    if Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("skipping Anthropic TUI smoke because tmux is unavailable");
        return Ok(());
    }

    let repo_root = codex_utils_cargo_bin::repo_root()?;
    let codex = codex_binary(&repo_root)?;
    let codex_home = tempdir()?;
    let server = wiremock::MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(anthropic_text_sse("msg-tui", "Anthropic TUI sentinel")),
        )
        .expect(1)
        .mount(&server)
        .await;
    write_config(codex_home.path(), &repo_root, &server.uri())?;

    let session_name = format!("codex-anthropic-tui-smoke-{}", std::process::id());
    let _session = TmuxSession {
        name: session_name.clone(),
    };

    let start_output = checked_output(
        Command::new("tmux")
            .arg("new-session")
            .arg("-d")
            .arg("-P")
            .arg("-F")
            .arg("#{pane_id}")
            .arg("-x")
            .arg("120")
            .arg("-y")
            .arg("40")
            .arg("-s")
            .arg(&session_name)
            .arg("--")
            .arg("env")
            .arg(format!("CODEX_HOME={}", codex_home.path().display()))
            .arg("ANTHROPIC_API_KEY=anthropic-test-key")
            .arg(codex)
            .arg("-c")
            .arg("analytics.enabled=false")
            .arg("--no-alt-screen")
            .arg("-C")
            .arg(&repo_root)
            .arg("Say hello through Anthropic."),
    )?;
    let codex_pane = stdout_text(&start_output).trim().to_string();
    anyhow::ensure!(!codex_pane.is_empty(), "tmux did not report a pane id");

    wait_for_capture_contains(
        &codex_pane,
        "Anthropic TUI sentinel",
        Duration::from_secs(/*secs*/ 15),
    )?;

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

    Ok(())
}

struct TmuxSession {
    name: String,
}

impl Drop for TmuxSession {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .arg("kill-session")
            .arg("-t")
            .arg(&self.name)
            .output();
    }
}

fn codex_binary(repo_root: &Path) -> Result<PathBuf> {
    if let Ok(path) = codex_utils_cargo_bin::cargo_bin("codex") {
        return Ok(path);
    }

    let fallback = repo_root.join("codex-rs/target/debug/codex");
    anyhow::ensure!(
        fallback.is_file(),
        "codex binary is unavailable; run `cargo build -p codex-cli` first"
    );
    Ok(fallback)
}

fn write_config(codex_home: &Path, repo_root: &Path, server_uri: &str) -> Result<()> {
    let repo_root_display = repo_root.display();
    let config = format!(
        r#"model = "claude-sonnet-4-6"
model_provider = "anthropic_test"
approval_policy = "never"
sandbox_mode = "read-only"
suppress_unstable_features_warning = true

[model_providers.anthropic_test]
name = "Anthropic Test"
base_url = "{server_uri}/v1"
env_key = "ANTHROPIC_API_KEY"
wire_api = "anthropic_messages"
request_max_retries = 0
stream_max_retries = 0

[model_providers.anthropic_test.http_headers]
anthropic-version = "2023-06-01"

[projects."{repo_root_display}"]
trust_level = "trusted"
"#
    );
    std::fs::write(codex_home.join("config.toml"), config)?;
    Ok(())
}

fn wait_for_capture_contains(pane: &str, needle: &str, timeout: Duration) -> Result<String> {
    let deadline = Instant::now() + timeout;
    let mut last_capture = String::new();
    while Instant::now() < deadline {
        last_capture = capture_pane(pane)?;
        if last_capture.contains(needle) {
            return Ok(last_capture);
        }
        sleep(Duration::from_millis(/*millis*/ 100));
    }

    anyhow::bail!("timed out waiting for {needle:?}; last capture:\n{last_capture}");
}

fn capture_pane(pane: &str) -> Result<String> {
    let output = output(
        Command::new("tmux")
            .arg("capture-pane")
            .arg("-p")
            .arg("-t")
            .arg(pane),
    )?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn checked_output(command: &mut Command) -> Result<Output> {
    let output = output(command)?;
    anyhow::ensure!(
        output.status.success(),
        "command failed with status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output)
}

fn output(command: &mut Command) -> Result<Output> {
    command
        .output()
        .with_context(|| format!("failed to run {command:?}"))
}

fn stdout_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
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
