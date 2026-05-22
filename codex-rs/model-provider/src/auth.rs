use std::sync::Arc;

use codex_agent_identity::AgentIdentityKey;
use codex_agent_identity::AgentTaskAuthorizationTarget;
use codex_agent_identity::authorization_header_for_agent_task;
use codex_api::AuthProvider;
use codex_api::SharedAuthProvider;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_model_provider_info::ModelProviderInfo;
use http::HeaderMap;
use http::HeaderValue;

use crate::bearer_auth_provider::BearerAuthProvider;

#[derive(Clone, Debug)]
struct AgentIdentityAuthProvider {
    auth: codex_login::auth::AgentIdentityAuth,
}

impl AuthProvider for AgentIdentityAuthProvider {
    fn add_auth_headers(&self, headers: &mut HeaderMap) {
        let record = self.auth.record();
        let header_value = authorization_header_for_agent_task(
            AgentIdentityKey {
                agent_runtime_id: &record.agent_runtime_id,
                private_key_pkcs8_base64: &record.agent_private_key,
            },
            AgentTaskAuthorizationTarget {
                agent_runtime_id: &record.agent_runtime_id,
                task_id: self.auth.process_task_id(),
            },
        )
        .map_err(std::io::Error::other);

        if let Ok(header_value) = header_value
            && let Ok(header) = HeaderValue::from_str(&header_value)
        {
            let _ = headers.insert(http::header::AUTHORIZATION, header);
        }

        if let Ok(header) = HeaderValue::from_str(self.auth.account_id()) {
            let _ = headers.insert("ChatGPT-Account-ID", header);
        }

        if self.auth.is_fedramp_account() {
            let _ = headers.insert("X-OpenAI-Fedramp", HeaderValue::from_static("true"));
        }
    }
}

// Some providers are meant to send no auth headers. Examples include local OSS
// providers and custom test providers with `requires_openai_auth = false`.
#[derive(Clone, Debug)]
struct UnauthenticatedAuthProvider;

impl AuthProvider for UnauthenticatedAuthProvider {
    fn add_auth_headers(&self, _headers: &mut HeaderMap) {}
}

pub fn unauthenticated_auth_provider() -> SharedAuthProvider {
    Arc::new(UnauthenticatedAuthProvider)
}

#[derive(Clone, Debug)]
struct ApiKeyHeaderAuthProvider {
    header_name: http::HeaderName,
    api_key: String,
}

impl ApiKeyHeaderAuthProvider {
    fn new(header_name: http::HeaderName, api_key: String) -> Self {
        Self {
            header_name,
            api_key,
        }
    }
}

impl AuthProvider for ApiKeyHeaderAuthProvider {
    fn add_auth_headers(&self, headers: &mut HeaderMap) {
        if let Ok(header) = HeaderValue::from_str(&self.api_key) {
            let _ = headers.insert(self.header_name.clone(), header);
        }
    }
}

/// Returns the provider-scoped auth manager when this provider uses command-backed auth.
///
/// Providers without custom auth continue using the caller-supplied base manager, when present.
pub(crate) fn auth_manager_for_provider(
    auth_manager: Option<Arc<AuthManager>>,
    provider: &ModelProviderInfo,
) -> Option<Arc<AuthManager>> {
    match provider.auth.clone() {
        Some(config) => Some(AuthManager::external_bearer_only(config)),
        None => auth_manager,
    }
}

pub(crate) fn resolve_provider_auth(
    auth: Option<&CodexAuth>,
    provider: &ModelProviderInfo,
) -> codex_protocol::error::Result<SharedAuthProvider> {
    if provider.wire_api == codex_model_provider_info::WireApi::AnthropicMessages
        && let Some(api_key) = provider.api_key()?
    {
        return Ok(Arc::new(ApiKeyHeaderAuthProvider::new(
            http::HeaderName::from_static("x-api-key"),
            api_key,
        )));
    }

    if let Some(auth) = bearer_auth_for_provider(provider)? {
        return Ok(Arc::new(auth));
    }

    Ok(match auth {
        Some(auth) => auth_provider_from_auth(auth),
        None => unauthenticated_auth_provider(),
    })
}

fn bearer_auth_for_provider(
    provider: &ModelProviderInfo,
) -> codex_protocol::error::Result<Option<BearerAuthProvider>> {
    if let Some(api_key) = provider.api_key()? {
        return Ok(Some(BearerAuthProvider::new(api_key)));
    }

    if let Some(token) = provider.experimental_bearer_token.clone() {
        return Ok(Some(BearerAuthProvider::new(token)));
    }

    Ok(None)
}

/// Builds request-header auth for a first-party Codex auth snapshot.
pub fn auth_provider_from_auth(auth: &CodexAuth) -> SharedAuthProvider {
    match auth {
        CodexAuth::AgentIdentity(auth) => {
            Arc::new(AgentIdentityAuthProvider { auth: auth.clone() })
        }
        CodexAuth::ApiKey(_) | CodexAuth::Chatgpt(_) | CodexAuth::ChatgptAuthTokens(_) => {
            Arc::new(BearerAuthProvider {
                token: auth.get_token().ok(),
                account_id: auth.get_account_id(),
                is_fedramp_account: auth.is_fedramp_account(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use codex_model_provider_info::WireApi;
    use codex_model_provider_info::create_oss_provider_with_base_url;
    use codex_protocol::error::CodexErr;

    use super::*;

    const EXISTING_ENV_VAR_WITH_NON_EMPTY_VALUE: &str = "PATH";

    #[test]
    fn unauthenticated_auth_provider_adds_no_headers() {
        let provider =
            create_oss_provider_with_base_url("http://localhost:11434/v1", WireApi::Responses);
        let auth = resolve_provider_auth(/*auth*/ None, &provider).expect("auth should resolve");

        assert!(auth.to_auth_headers().is_empty());
    }

    #[test]
    fn api_key_header_auth_provider_adds_x_api_key_header() {
        let auth = ApiKeyHeaderAuthProvider::new(
            http::HeaderName::from_static("x-api-key"),
            "anthropic-key".to_string(),
        );
        let headers = auth.to_auth_headers();

        assert_eq!(
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("anthropic-key")
        );
        assert!(!headers.contains_key(http::header::AUTHORIZATION));
    }

    #[test]
    fn anthropic_auth_requires_configured_env_key() {
        let mut provider = ModelProviderInfo::create_anthropic_provider();
        let env_key = format!(
            "CODEX_TEST_MISSING_ANTHROPIC_API_KEY_{}",
            std::process::id()
        );
        provider.env_key = Some(env_key.clone());

        let err = match resolve_provider_auth(/*auth*/ None, &provider) {
            Ok(_) => panic!("missing Anthropic env key should fail auth resolution"),
            Err(err) => err,
        };

        match err {
            CodexErr::EnvVar(error) => {
                assert_eq!(error.var, env_key);
                assert_eq!(
                    error.instructions.as_deref(),
                    Some("Set ANTHROPIC_API_KEY to an Anthropic Console API key.")
                );
            }
            other => panic!("expected env var error, got {other:?}"),
        }
    }

    #[test]
    fn anthropic_auth_uses_x_api_key_from_env_key() {
        let mut provider = ModelProviderInfo::create_anthropic_provider();
        provider.env_key = Some(EXISTING_ENV_VAR_WITH_NON_EMPTY_VALUE.to_string());
        let expected = std::env::var(EXISTING_ENV_VAR_WITH_NON_EMPTY_VALUE)
            .expect("test env var should be present");

        let auth = resolve_provider_auth(/*auth*/ None, &provider).expect("auth should resolve");
        let headers = auth.to_auth_headers();

        assert_eq!(
            headers
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some(expected.as_str())
        );
        assert!(!headers.contains_key(http::header::AUTHORIZATION));
    }
}
