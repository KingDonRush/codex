use std::time::Duration;

use anyhow::Result;
use app_test_support::McpProcess;
use app_test_support::to_response;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::ModelProviderCapabilitiesReadParams;
use codex_app_server_protocol::ModelProviderCapabilitiesReadResponse;
use codex_app_server_protocol::RequestId;
use pretty_assertions::assert_eq;
use std::path::Path;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

async fn read_capabilities(codex_home: &Path) -> Result<ModelProviderCapabilitiesReadResponse> {
    let mut mcp = McpProcess::new(codex_home).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_model_provider_capabilities_read_request(ModelProviderCapabilitiesReadParams {})
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    to_response(response)
}

#[tokio::test]
async fn read_default_provider_capabilities() -> Result<()> {
    let codex_home = TempDir::new()?;
    let received = read_capabilities(codex_home.path()).await?;

    let expected = ModelProviderCapabilitiesReadResponse {
        namespace_tools: true,
        image_generation: true,
        web_search: true,
    };
    assert_eq!(received, expected);
    Ok(())
}

#[tokio::test]
async fn read_amazon_bedrock_provider_capabilities() -> Result<()> {
    let codex_home = TempDir::new()?;
    std::fs::write(
        codex_home.path().join("config.toml"),
        r#"model_provider = "amazon-bedrock"
"#,
    )?;
    let received = read_capabilities(codex_home.path()).await?;

    let expected = ModelProviderCapabilitiesReadResponse {
        namespace_tools: false,
        image_generation: false,
        web_search: false,
    };
    assert_eq!(received, expected);
    Ok(())
}

#[tokio::test]
async fn read_anthropic_provider_capabilities() -> Result<()> {
    let codex_home = TempDir::new()?;
    std::fs::write(
        codex_home.path().join("config.toml"),
        r#"model_provider = "anthropic"
"#,
    )?;
    let received = read_capabilities(codex_home.path()).await?;

    let expected = ModelProviderCapabilitiesReadResponse {
        namespace_tools: true,
        image_generation: false,
        web_search: true,
    };
    assert_eq!(received, expected);
    Ok(())
}
