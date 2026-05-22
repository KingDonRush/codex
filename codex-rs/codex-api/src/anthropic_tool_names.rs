use std::collections::HashMap;
use std::collections::HashSet;

const MAX_ANTHROPIC_TOOL_NAME_LEN: usize = 64;
const HASH_SUFFIX_LEN: usize = 17;
const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct AnthropicToolNameMap {
    by_anthropic: HashMap<String, AnthropicToolName>,
    by_internal: HashMap<InternalToolName, String>,
    reserved: HashSet<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AnthropicToolName {
    pub(crate) namespace: Option<String>,
    pub(crate) name: String,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct InternalToolName {
    namespace: Option<String>,
    name: String,
}

impl AnthropicToolNameMap {
    pub(crate) fn register(&mut self, namespace: Option<String>, name: String) -> String {
        self.register_with_candidate(namespace, name, None)
    }

    pub(crate) fn register_with_anthropic_name(
        &mut self,
        namespace: Option<String>,
        name: String,
        anthropic_name: &str,
    ) -> String {
        self.register_with_candidate(namespace, name, Some(anthropic_name))
    }

    fn register_with_candidate(
        &mut self,
        namespace: Option<String>,
        name: String,
        anthropic_name: Option<&str>,
    ) -> String {
        let internal = InternalToolName { namespace, name };
        if let Some(registered) = self.by_internal.get(&internal) {
            return registered.clone();
        }

        let candidate = anthropic_name
            .map(normalize_tool_name)
            .unwrap_or_else(|| candidate_tool_name(internal.namespace.as_deref(), &internal.name));
        let name = self.unique_name(&candidate, &internal);
        self.reserved.insert(name.clone());
        self.by_anthropic.insert(
            name.clone(),
            AnthropicToolName {
                namespace: internal.namespace.clone(),
                name: internal.name.clone(),
            },
        );
        self.by_internal.insert(internal, name.clone());
        name
    }

    pub(crate) fn anthropic_name(&self, namespace: Option<&str>, name: &str) -> String {
        let internal = InternalToolName {
            namespace: namespace.map(str::to_string),
            name: name.to_string(),
        };
        self.by_internal
            .get(&internal)
            .cloned()
            .unwrap_or_else(|| candidate_tool_name(namespace, name))
    }

    pub(crate) fn resolve(&self, anthropic_name: &str) -> Option<AnthropicToolName> {
        if let Some(tool_name) = self.by_anthropic.get(anthropic_name) {
            return Some(tool_name.clone());
        }

        self.by_anthropic.is_empty().then(|| AnthropicToolName {
            namespace: None,
            name: anthropic_name.to_string(),
        })
    }

    fn unique_name(&self, candidate: &str, internal: &InternalToolName) -> String {
        if !self.reserved.contains(candidate) {
            return candidate.to_string();
        }

        let hash = stable_hash(&internal_hash_source(internal));
        with_hash_suffix(candidate, hash)
    }
}

fn candidate_tool_name(namespace: Option<&str>, name: &str) -> String {
    let raw_name = match namespace {
        Some(namespace) if namespace.ends_with('_') || name.starts_with('_') => {
            format!("{namespace}{name}")
        }
        Some(namespace) => format!("{namespace}_{name}"),
        None => name.to_string(),
    };

    normalize_tool_name(&raw_name)
}

fn normalize_tool_name(raw_name: &str) -> String {
    let mut normalized = String::with_capacity(raw_name.len());
    for char in raw_name.chars() {
        if char.is_ascii_alphanumeric() || char == '_' || char == '-' {
            normalized.push(char);
        } else {
            normalized.push('_');
        }
    }

    if normalized.is_empty() {
        normalized.push_str("tool");
    }

    if normalized.len() > MAX_ANTHROPIC_TOOL_NAME_LEN {
        let hash = stable_hash(raw_name);
        normalized = with_hash_suffix(&normalized, hash);
    }

    normalized
}

fn with_hash_suffix(name: &str, hash: u64) -> String {
    let max_prefix_len = MAX_ANTHROPIC_TOOL_NAME_LEN - HASH_SUFFIX_LEN;
    let prefix_len = name.len().min(max_prefix_len);
    format!("{}_{hash:016x}", &name[..prefix_len])
}

fn internal_hash_source(internal: &InternalToolName) -> String {
    format!(
        "{}\u{1f}{}",
        internal.namespace.as_deref().unwrap_or_default(),
        internal.name
    )
}

fn stable_hash(value: &str) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_namespaced_tool_names() {
        let mut map = AnthropicToolNameMap::default();

        let anthropic_name =
            map.register(Some("mcp__demo__".to_string()), "lookup_order".to_string());

        assert_eq!(anthropic_name, "mcp__demo__lookup_order");
        assert_eq!(
            map.resolve(&anthropic_name),
            Some(AnthropicToolName {
                namespace: Some("mcp__demo__".to_string()),
                name: "lookup_order".to_string(),
            })
        );
    }

    #[test]
    fn keeps_colliding_internal_names_reversible() {
        let mut map = AnthropicToolNameMap::default();

        let plain = map.register(None, "mcp__demo__lookup_order".to_string());
        let namespaced = map.register(Some("mcp__demo__".to_string()), "lookup_order".to_string());

        assert_ne!(plain, namespaced);
        assert_eq!(
            map.resolve(&plain),
            Some(AnthropicToolName {
                namespace: None,
                name: "mcp__demo__lookup_order".to_string(),
            })
        );
        assert_eq!(
            map.resolve(&namespaced),
            Some(AnthropicToolName {
                namespace: Some("mcp__demo__".to_string()),
                name: "lookup_order".to_string(),
            })
        );
    }

    #[test]
    fn resolves_unknown_names_only_when_map_is_empty() {
        let mut map = AnthropicToolNameMap::default();

        assert_eq!(
            map.resolve("shell"),
            Some(AnthropicToolName {
                namespace: None,
                name: "shell".to_string(),
            })
        );

        map.register(None, "known".to_string());

        assert_eq!(map.resolve("shell"), None);
    }

    #[test]
    fn normalizes_names_to_anthropic_constraints() {
        let mut map = AnthropicToolNameMap::default();

        let anthropic_name = map.register(Some("mcp demo".to_string()), "lookup/order".to_string());

        assert_eq!(anthropic_name, "mcp_demo_lookup_order");
    }

    #[test]
    fn truncates_long_names_with_stable_suffix() {
        let mut map = AnthropicToolNameMap::default();

        let anthropic_name = map.register(None, "x".repeat(100));

        assert_eq!(anthropic_name.len(), MAX_ANTHROPIC_TOOL_NAME_LEN);
        assert!(anthropic_name.starts_with("xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx_"));
    }

    #[test]
    fn registers_explicit_anthropic_name_for_reserved_tools() {
        let mut map = AnthropicToolNameMap::default();

        let anthropic_name =
            map.register_with_anthropic_name(None, "tool_search".to_string(), "codex_tool_search");

        assert_eq!(anthropic_name, "codex_tool_search");
        assert_eq!(
            map.resolve("codex_tool_search"),
            Some(AnthropicToolName {
                namespace: None,
                name: "tool_search".to_string(),
            })
        );
    }
}
