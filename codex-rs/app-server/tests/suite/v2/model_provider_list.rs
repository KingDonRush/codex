use std::time::Duration;

use anyhow::Result;
use app_test_support::McpProcess;
use app_test_support::to_response;
use codex_app_server_protocol::JSONRPCError;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::ModelProviderCapabilities;
use codex_app_server_protocol::ModelProviderEnvHttpHeader;
use codex_app_server_protocol::ModelProviderListParams;
use codex_app_server_protocol::ModelProviderListResponse;
use codex_app_server_protocol::ModelProviderSummary;
use codex_app_server_protocol::RequestId;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const INVALID_REQUEST_ERROR_CODE: i64 = -32600;

async fn list_model_providers(
    mcp: &mut McpProcess,
    params: ModelProviderListParams,
) -> Result<ModelProviderListResponse> {
    let request_id = mcp.send_model_provider_list_request(params).await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;

    to_response(response)
}

#[tokio::test]
async fn list_model_providers_returns_sanitized_built_ins() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let ModelProviderListResponse { data, next_cursor } = list_model_providers(
        &mut mcp,
        ModelProviderListParams {
            cursor: None,
            limit: Some(100),
        },
    )
    .await?;

    assert!(next_cursor.is_none());
    assert_eq!(
        data.iter()
            .map(|provider| provider.id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "amazon-bedrock",
            "anthropic",
            "lmstudio",
            "ollama",
            "openai"
        ]
    );

    let anthropic = data
        .iter()
        .find(|provider| provider.id == "anthropic")
        .expect("anthropic provider should be present");
    let expected_anthropic = ModelProviderSummary {
        id: "anthropic".to_string(),
        name: "Anthropic".to_string(),
        base_url: Some("https://api.anthropic.com/v1".to_string()),
        wire_api: "anthropic_messages".to_string(),
        is_active: false,
        requires_openai_auth: false,
        supports_websockets: false,
        env_key: Some("ANTHROPIC_API_KEY".to_string()),
        env_http_headers: Vec::new(),
        static_http_headers: vec!["anthropic-version".to_string()],
        has_static_bearer_token: false,
        has_command_auth: false,
        capabilities: ModelProviderCapabilities {
            namespace_tools: true,
            image_generation: false,
            web_search: true,
        },
    };
    assert_eq!(anthropic, &expected_anthropic);

    let openai = data
        .iter()
        .find(|provider| provider.id == "openai")
        .expect("openai provider should be present");
    assert!(openai.is_active);
    assert_eq!(openai.wire_api, "responses");
    assert_eq!(
        openai.env_http_headers,
        vec![
            ModelProviderEnvHttpHeader {
                header: "OpenAI-Organization".to_string(),
                env_var: "OPENAI_ORGANIZATION".to_string(),
            },
            ModelProviderEnvHttpHeader {
                header: "OpenAI-Project".to_string(),
                env_var: "OPENAI_PROJECT".to_string(),
            },
        ]
    );
    assert_eq!(openai.static_http_headers, vec!["version".to_string()]);
    Ok(())
}

#[tokio::test]
async fn list_model_providers_redacts_custom_provider_secrets() -> Result<()> {
    let codex_home = TempDir::new()?;
    std::fs::write(
        codex_home.path().join("config.toml"),
        r#"
model_provider = "claudee"

[model_providers.claudee]
name = "GGMAX / Claudee"
base_url = "https://s1.claudee.pro/v1"
env_key = "CLAUDEE_API_KEY"
wire_api = "anthropic_messages"
experimental_bearer_token = "secret-token"

[model_providers.claudee.http_headers]
Authorization = "Bearer secret"
anthropic-version = "2023-06-01"

[model_providers.claudee.env_http_headers]
Authorization = "CLAUDEE_AUTHORIZATION"
"#,
    )?;
    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let response = list_model_providers(
        &mut mcp,
        ModelProviderListParams {
            cursor: None,
            limit: Some(100),
        },
    )
    .await?;

    let response_json = serde_json::to_string(&response)?;
    assert!(!response_json.contains("secret-token"));
    assert!(!response_json.contains("Bearer secret"));
    assert!(!response_json.contains("2023-06-01"));

    let claudee = response
        .data
        .iter()
        .find(|provider| provider.id == "claudee")
        .expect("custom provider should be present");
    let expected = ModelProviderSummary {
        id: "claudee".to_string(),
        name: "GGMAX / Claudee".to_string(),
        base_url: Some("https://s1.claudee.pro/v1".to_string()),
        wire_api: "anthropic_messages".to_string(),
        is_active: true,
        requires_openai_auth: false,
        supports_websockets: false,
        env_key: Some("CLAUDEE_API_KEY".to_string()),
        env_http_headers: vec![ModelProviderEnvHttpHeader {
            header: "Authorization".to_string(),
            env_var: "CLAUDEE_AUTHORIZATION".to_string(),
        }],
        static_http_headers: vec!["Authorization".to_string(), "anthropic-version".to_string()],
        has_static_bearer_token: true,
        has_command_auth: false,
        capabilities: ModelProviderCapabilities {
            namespace_tools: true,
            image_generation: false,
            web_search: true,
        },
    };
    assert_eq!(claudee, &expected);
    assert!(response.next_cursor.is_none());
    Ok(())
}

#[tokio::test]
async fn list_model_providers_pagination_works() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let first_page = list_model_providers(
        &mut mcp,
        ModelProviderListParams {
            cursor: None,
            limit: Some(2),
        },
    )
    .await?;
    assert_eq!(first_page.data.len(), 2);
    assert_eq!(first_page.next_cursor, Some("2".to_string()));

    let second_page = list_model_providers(
        &mut mcp,
        ModelProviderListParams {
            cursor: first_page.next_cursor,
            limit: Some(100),
        },
    )
    .await?;
    assert_eq!(second_page.data.len(), 3);
    assert!(second_page.next_cursor.is_none());
    Ok(())
}

#[tokio::test]
async fn list_model_providers_rejects_invalid_cursor() -> Result<()> {
    let codex_home = TempDir::new()?;
    let mut mcp = McpProcess::new(codex_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_model_provider_list_request(ModelProviderListParams {
            cursor: Some("invalid".to_string()),
            limit: None,
        })
        .await?;

    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INVALID_REQUEST_ERROR_CODE);
    assert_eq!(error.error.message, "invalid cursor: invalid");
    Ok(())
}
