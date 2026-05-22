mod catalog;

use std::path::PathBuf;
use std::sync::Arc;

use codex_api::Provider;
use codex_api::SharedAuthProvider;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_model_provider_info::ANTHROPIC_DEFAULT_MODEL_ID;
use codex_model_provider_info::ModelProviderInfo;
use codex_models_manager::manager::SharedModelsManager;
use codex_models_manager::manager::StaticModelsManager;
use codex_protocol::account::ProviderAccount;
use codex_protocol::error::Result;
use codex_protocol::openai_models::ModelsResponse;

use crate::auth::resolve_provider_auth;
use crate::provider::ModelProvider;
use crate::provider::ProviderAccountResult;
use crate::provider::ProviderAccountState;
use crate::provider::ProviderCapabilities;
pub(crate) use catalog::static_model_catalog;

/// Runtime provider for Anthropic's native Messages API.
#[derive(Clone, Debug)]
pub(crate) struct AnthropicModelProvider {
    info: ModelProviderInfo,
}

impl AnthropicModelProvider {
    pub(crate) fn new(provider_info: ModelProviderInfo) -> Self {
        Self {
            info: provider_info,
        }
    }
}

#[async_trait::async_trait]
impl ModelProvider for AnthropicModelProvider {
    fn info(&self) -> &ModelProviderInfo {
        &self.info
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            namespace_tools: false,
            image_generation: false,
            web_search: false,
        }
    }

    fn approval_review_preferred_model(&self) -> &'static str {
        ANTHROPIC_DEFAULT_MODEL_ID
    }

    fn auth_manager(&self) -> Option<Arc<AuthManager>> {
        None
    }

    async fn auth(&self) -> Option<CodexAuth> {
        None
    }

    fn account_state(&self) -> ProviderAccountResult {
        Ok(ProviderAccountState {
            account: Some(ProviderAccount::ApiKey),
            requires_openai_auth: false,
        })
    }

    async fn api_provider(&self) -> Result<Provider> {
        self.info.to_api_provider(/*auth_mode*/ None)
    }

    async fn api_auth(&self) -> Result<SharedAuthProvider> {
        resolve_provider_auth(/*auth*/ None, &self.info)
    }

    fn models_manager(
        &self,
        _codex_home: PathBuf,
        config_model_catalog: Option<ModelsResponse>,
    ) -> SharedModelsManager {
        Arc::new(StaticModelsManager::new(
            /*auth_manager*/ None,
            config_model_catalog.unwrap_or_else(static_model_catalog),
        ))
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn capabilities_disable_unimplemented_native_features() {
        let provider = AnthropicModelProvider::new(ModelProviderInfo::create_anthropic_provider());

        assert_eq!(
            provider.capabilities(),
            ProviderCapabilities {
                namespace_tools: false,
                image_generation: false,
                web_search: false,
            }
        );
    }
}
