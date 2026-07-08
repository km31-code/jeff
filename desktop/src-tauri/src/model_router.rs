// apex a1: the model router. every llm call site declares a capability tier;
// the router maps tiers to provider/model pairs from persisted config and
// dispatches to the matching adapter. call sites never name models — the
// done-when gate for this milestone greps for model strings outside this
// module and providers/.
//
// tiers:
//   reflex       — classification, cheap tagging. fast + cheap above all.
//   conversation — chat turns, streaming. fast frontier.
//   judgment     — synthesis decisions, proactive wording. mid frontier.
//   craft        — drafting, revision, planning. top frontier.

use std::sync::{Arc, RwLock};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::providers::anthropic::{self, AnthropicRequest};
use crate::store::TaskStore;

pub const TIER_MODEL_MAP_SETTING: &str = "tier_model_map";

// default models per tier. reflex stays on the openai fast model until the
// local runtime lands in a3. judgment and craft default to anthropic because
// the character spec is written for a model that can hold a voice.
pub const DEFAULT_REFLEX_MODEL: &str = "gpt-4o-mini";
pub const DEFAULT_CONVERSATION_MODEL: &str = "claude-haiku-4-5";
pub const DEFAULT_JUDGMENT_MODEL: &str = "claude-sonnet-5";
pub const DEFAULT_CRAFT_MODEL: &str = "claude-sonnet-5";
// used when an anthropic tier must fall back because no anthropic key exists.
pub const OPENAI_FALLBACK_MODEL: &str = "gpt-4o-mini";

const CLASSIFY_TIMEOUT_OPENAI_MS: u64 = 300;
const CLASSIFY_TIMEOUT_ANTHROPIC_MS: u64 = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Reflex,
    Conversation,
    Judgment,
    Craft,
}

impl Tier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Tier::Reflex => "reflex",
            Tier::Conversation => "conversation",
            Tier::Judgment => "judgment",
            Tier::Craft => "craft",
        }
    }
}

// cache hints are plumbed in a1 and consumed by the a2 cache-stable prompt
// milestone. stable blocks must be byte-identical across turns; session
// blocks change rarely; volatile blocks change every turn.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheHint {
    Stable,
    Session,
    Volatile,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SystemBlock {
    pub text: String,
    pub cache_hint: CacheHint,
}

#[allow(dead_code)]
pub fn join_system_blocks(blocks: &[SystemBlock]) -> String {
    blocks
        .iter()
        .map(|block| block.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct LlmUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelMessageRole {
    User,
    Assistant,
}

impl ModelMessageRole {
    fn as_prompt_label(&self) -> &'static str {
        match self {
            ModelMessageRole::User => "user",
            ModelMessageRole::Assistant => "assistant",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelMessage {
    pub role: ModelMessageRole,
    pub content: String,
}

impl ModelMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ModelMessageRole::User,
            content: content.into(),
        }
    }

    #[allow(dead_code)]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ModelMessageRole::Assistant,
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModelRequest {
    pub tier: Tier,
    pub system_blocks: Vec<SystemBlock>,
    pub messages: Vec<ModelMessage>,
    pub json_schema: Option<serde_json::Value>,
    pub stream: bool,
    pub max_tokens: Option<u32>,
    pub temperature: f32,
    pub timeout_ms: Option<u64>,
    #[allow(dead_code)]
    pub purpose: Option<String>,
}

impl ModelRequest {
    pub fn new(tier: Tier, system: impl Into<String>, user: impl Into<String>) -> Self {
        Self {
            tier,
            system_blocks: vec![SystemBlock {
                text: system.into(),
                cache_hint: CacheHint::Volatile,
            }],
            messages: vec![ModelMessage::user(user)],
            json_schema: None,
            stream: false,
            max_tokens: None,
            temperature: 0.0,
            timeout_ms: None,
            purpose: None,
        }
    }

    pub fn with_options(mut self, options: GenerateOptions) -> Self {
        self.temperature = options.temperature;
        self.max_tokens = options.max_tokens;
        self.timeout_ms = options.timeout_ms;
        if options.json_object {
            self.json_schema = Some(serde_json::json!({ "type": "json_object" }));
        }
        self
    }
}

#[derive(Debug, Clone)]
pub struct ModelResponse {
    pub text: String,
    #[allow(dead_code)]
    pub usage: LlmUsage,
    #[allow(dead_code)]
    pub provider: ProviderKind,
    #[allow(dead_code)]
    pub model: String,
    #[allow(dead_code)]
    pub tier: Tier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    OpenAi,
    Anthropic,
}

impl ProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "openai",
            ProviderKind::Anthropic => "anthropic",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierConfig {
    pub provider: ProviderKind,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouterConfig {
    pub reflex: TierConfig,
    pub conversation: TierConfig,
    pub judgment: TierConfig,
    pub craft: TierConfig,
}

impl Default for RouterConfig {
    fn default() -> Self {
        Self {
            reflex: TierConfig {
                provider: ProviderKind::OpenAi,
                model: DEFAULT_REFLEX_MODEL.to_string(),
            },
            conversation: TierConfig {
                provider: ProviderKind::Anthropic,
                model: DEFAULT_CONVERSATION_MODEL.to_string(),
            },
            judgment: TierConfig {
                provider: ProviderKind::Anthropic,
                model: DEFAULT_JUDGMENT_MODEL.to_string(),
            },
            craft: TierConfig {
                provider: ProviderKind::Anthropic,
                model: DEFAULT_CRAFT_MODEL.to_string(),
            },
        }
    }
}

impl RouterConfig {
    pub fn for_tier(&self, tier: Tier) -> &TierConfig {
        match tier {
            Tier::Reflex => &self.reflex,
            Tier::Conversation => &self.conversation,
            Tier::Judgment => &self.judgment,
            Tier::Craft => &self.craft,
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        let config: RouterConfig =
            serde_json::from_str(raw).context("failed to parse tier_model_map JSON")?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        for (tier, cfg) in [
            (Tier::Reflex, &self.reflex),
            (Tier::Conversation, &self.conversation),
            (Tier::Judgment, &self.judgment),
            (Tier::Craft, &self.craft),
        ] {
            if cfg.model.trim().is_empty() {
                return Err(anyhow!("tier {} has an empty model name", tier.as_str()));
            }
        }
        Ok(())
    }
}

// options for non-default generation parameters. call sites that need custom
// temperature, output budget, json mode, or timeout pass these explicitly.
#[derive(Debug, Clone, Copy, Default)]
pub struct GenerateOptions {
    pub temperature: f32,
    pub max_tokens: Option<u32>,
    pub json_object: bool,
    pub timeout_ms: Option<u64>,
}

pub struct ModelRouter {
    config: RwLock<RouterConfig>,
}

impl ModelRouter {
    pub fn new(config: RouterConfig) -> Self {
        Self {
            config: RwLock::new(config),
        }
    }

    // loads persisted config from app_settings, falling back to defaults on
    // absence or parse failure. a bad stored config never blocks startup.
    pub fn from_store(store: &TaskStore) -> Self {
        let config = match store.get_app_setting(TIER_MODEL_MAP_SETTING) {
            Ok(Some(raw)) => match RouterConfig::parse(&raw) {
                Ok(parsed) => parsed,
                Err(err) => {
                    eprintln!(
                        "[jeff] model_router_config_invalid: {err}; using defaults"
                    );
                    RouterConfig::default()
                }
            },
            _ => RouterConfig::default(),
        };
        Self::new(config)
    }

    pub fn config(&self) -> RouterConfig {
        self.config
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    pub fn set_config(&self, config: RouterConfig, store: &TaskStore) -> Result<()> {
        config.validate()?;
        let raw = serde_json::to_string(&config).context("failed to serialize router config")?;
        store.set_app_setting(TIER_MODEL_MAP_SETTING, &raw)?;
        if let Ok(mut guard) = self.config.write() {
            *guard = config;
        }
        Ok(())
    }

    pub fn any_key_available(&self) -> bool {
        crate::secrets::resolve_openai_api_key().api_key.is_some()
            || crate::secrets::resolve_anthropic_api_key().is_some()
    }

    // resolves the effective provider/model for a tier, applying the
    // missing-key fallback: an anthropic tier with no anthropic key falls
    // back to openai with a logged notice rather than failing.
    pub fn resolve(&self, tier: Tier) -> TierConfig {
        let configured = self.config().for_tier(tier).clone();
        match configured.provider {
            ProviderKind::Anthropic => {
                if crate::secrets::resolve_anthropic_api_key().is_some() {
                    configured
                } else {
                    eprintln!(
                        "[jeff] model_router_fallback tier={} reason=missing_anthropic_key model={}",
                        tier.as_str(),
                        OPENAI_FALLBACK_MODEL
                    );
                    TierConfig {
                        provider: ProviderKind::OpenAi,
                        model: OPENAI_FALLBACK_MODEL.to_string(),
                    }
                }
            }
            ProviderKind::OpenAi => configured,
        }
    }

    fn log_usage(&self, tier: Tier, cfg: &TierConfig, usage: &LlmUsage) {
        eprintln!(
            "[jeff] llm_usage tier={} provider={} model={} input={} output={} cached={}",
            tier.as_str(),
            cfg.provider.as_str(),
            cfg.model,
            usage.input_tokens,
            usage.output_tokens,
            usage.cached_tokens
        );
    }

    fn request_user_text(request: &ModelRequest) -> Result<String> {
        let non_empty = request
            .messages
            .iter()
            .filter_map(|message| {
                let content = message.content.trim();
                (!content.is_empty()).then_some((message.role.clone(), content.to_string()))
            })
            .collect::<Vec<_>>();

        if non_empty.is_empty() {
            return Err(anyhow!("model request contains no message content"));
        }

        if non_empty.len() == 1 && non_empty[0].0 == ModelMessageRole::User {
            return Ok(non_empty[0].1.clone());
        }

        Ok(non_empty
            .into_iter()
            .map(|(role, content)| format!("{}: {}", role.as_prompt_label(), content))
            .collect::<Vec<_>>()
            .join("\n\n"))
    }

    pub fn route(&self, request: ModelRequest) -> Result<ModelResponse> {
        if request.stream {
            return Err(anyhow!(
                "streaming model requests must use route_streaming"
            ));
        }

        let system = join_system_blocks(&request.system_blocks);
        let user = Self::request_user_text(&request)?;
        let cfg = self.resolve(request.tier);
        let json_object = request.json_schema.is_some();
        let (text, usage) = match cfg.provider {
            ProviderKind::OpenAi => crate::providers::openai_generate_blocking(
                &cfg.model,
                &system,
                &user,
                request.temperature,
                request.max_tokens,
                json_object,
                request.timeout_ms,
            )?,
            ProviderKind::Anthropic => anthropic::generate_blocking(&AnthropicRequest {
                model: &cfg.model,
                system: &system,
                user: &user,
                temperature: request.temperature,
                max_tokens: request.max_tokens,
                json_only: json_object,
                timeout_ms: request.timeout_ms,
            })?,
        };
        self.log_usage(request.tier, &cfg, &usage);
        Ok(ModelResponse {
            text,
            usage,
            provider: cfg.provider,
            model: cfg.model,
            tier: request.tier,
        })
    }

    #[allow(dead_code)]
    pub fn route_streaming(
        &self,
        mut request: ModelRequest,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<Result<String>>> {
        request.stream = true;
        let system = join_system_blocks(&request.system_blocks);
        let user = Self::request_user_text(&request)?;
        self.stream(request.tier, &system, &user, cancel)
    }

    // blocking generation with default options (temperature 0, no cap).
    pub fn generate(&self, tier: Tier, system: &str, user: &str) -> Result<String> {
        self.generate_with(tier, system, user, GenerateOptions::default())
    }

    pub fn generate_with(
        &self,
        tier: Tier,
        system: &str,
        user: &str,
        options: GenerateOptions,
    ) -> Result<String> {
        Ok(self
            .route(ModelRequest::new(tier, system, user).with_options(options))?
            .text)
    }

    pub async fn generate_async(
        &self,
        tier: Tier,
        system: &str,
        user: &str,
        options: GenerateOptions,
    ) -> Result<String> {
        let cfg = self.resolve(tier);
        let (text, usage) = match cfg.provider {
            ProviderKind::OpenAi => {
                crate::providers::openai_generate_async(
                    &cfg.model,
                    system,
                    user,
                    options.temperature,
                    options.max_tokens,
                    options.json_object,
                    options.timeout_ms,
                )
                .await?
            }
            ProviderKind::Anthropic => {
                anthropic::generate_async(&AnthropicRequest {
                    model: &cfg.model,
                    system,
                    user,
                    temperature: options.temperature,
                    max_tokens: options.max_tokens,
                    json_only: options.json_object,
                    timeout_ms: options.timeout_ms,
                })
                .await?
            }
        };
        self.log_usage(tier, &cfg, &usage);
        Ok(text)
    }

    // streaming generation with the same channel contract as the legacy
    // openai streaming provider: text deltas until the channel closes.
    pub fn stream(
        &self,
        tier: Tier,
        system: &str,
        user: &str,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<Result<String>>> {
        let cfg = self.resolve(tier);
        eprintln!(
            "[jeff] llm_stream_open tier={} provider={} model={}",
            tier.as_str(),
            cfg.provider.as_str(),
            cfg.model
        );
        match cfg.provider {
            ProviderKind::OpenAi => crate::reasoning::OpenAiStreamingReasoningProvider::with_model(
                cfg.model.clone(),
            )
            .stream_response(system, user, cancel),
            ProviderKind::Anthropic => anthropic::stream(
                cfg.model.clone(),
                system.to_string(),
                user.to_string(),
                cancel,
            ),
        }
    }

    // reflex-tier intent classification. resolves keys internally and honors
    // a tight timeout so the frontend fallback budget is never regressed.
    pub fn classify(&self, text: &str) -> Result<crate::models::IntentClassificationDto> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(crate::models::IntentClassificationDto {
                intent: crate::models::IntentLabel::Unknown,
                confidence: 0.0,
                slots: crate::models::IntentSlotsDto::default(),
            });
        }
        let cfg = self.resolve(Tier::Reflex);
        let timeout_ms = match cfg.provider {
            ProviderKind::OpenAi => CLASSIFY_TIMEOUT_OPENAI_MS,
            ProviderKind::Anthropic => CLASSIFY_TIMEOUT_ANTHROPIC_MS,
        };
        let raw = self.generate_with(
            Tier::Reflex,
            crate::classifier::SYSTEM_PROMPT,
            trimmed,
            GenerateOptions {
                temperature: 0.0,
                max_tokens: Some(300),
                json_object: true,
                timeout_ms: Some(timeout_ms),
            },
        )?;
        crate::classifier::parse_classification(&raw)
    }

    // returns a tier-bound handle implementing the legacy reasoning trait so
    // existing function signatures (and their tests) keep working unchanged.
    pub fn handle(self: &Arc<Self>, tier: Tier) -> Arc<dyn crate::providers::ReasoningModelProvider> {
        Arc::new(TierHandle {
            router: Arc::clone(self),
            tier,
        })
    }
}

struct TierHandle {
    router: Arc<ModelRouter>,
    tier: Tier,
}

impl crate::providers::ReasoningModelProvider for TierHandle {
    fn generate_response(&self, system_prompt: &str, user_prompt: &str) -> Result<String> {
        self.router.generate(self.tier, system_prompt, user_prompt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_spec() {
        let config = RouterConfig::default();
        assert_eq!(config.reflex.provider, ProviderKind::OpenAi);
        assert_eq!(config.reflex.model, DEFAULT_REFLEX_MODEL);
        assert_eq!(config.conversation.provider, ProviderKind::Anthropic);
        assert_eq!(config.conversation.model, DEFAULT_CONVERSATION_MODEL);
        assert_eq!(config.judgment.provider, ProviderKind::Anthropic);
        assert_eq!(config.judgment.model, DEFAULT_JUDGMENT_MODEL);
        assert_eq!(config.craft.provider, ProviderKind::Anthropic);
        assert_eq!(config.craft.model, DEFAULT_CRAFT_MODEL);
    }

    #[test]
    fn config_round_trips_through_json() {
        let config = RouterConfig::default();
        let raw = serde_json::to_string(&config).unwrap();
        let parsed = RouterConfig::parse(&raw).unwrap();
        assert_eq!(parsed, config);
    }

    #[test]
    fn config_rejects_unknown_provider() {
        let raw = r#"{
            "reflex": {"provider": "openai", "model": "gpt-4o-mini"},
            "conversation": {"provider": "not-a-provider", "model": "x"},
            "judgment": {"provider": "anthropic", "model": "claude-sonnet-5"},
            "craft": {"provider": "anthropic", "model": "claude-sonnet-5"}
        }"#;
        assert!(RouterConfig::parse(raw).is_err());
    }

    #[test]
    fn config_rejects_empty_model_name() {
        let raw = r#"{
            "reflex": {"provider": "openai", "model": ""},
            "conversation": {"provider": "anthropic", "model": "claude-haiku-4-5"},
            "judgment": {"provider": "anthropic", "model": "claude-sonnet-5"},
            "craft": {"provider": "anthropic", "model": "claude-sonnet-5"}
        }"#;
        assert!(RouterConfig::parse(raw).is_err());
    }

    #[test]
    fn for_tier_returns_matching_config() {
        let config = RouterConfig::default();
        assert_eq!(config.for_tier(Tier::Reflex).model, DEFAULT_REFLEX_MODEL);
        assert_eq!(config.for_tier(Tier::Craft).model, DEFAULT_CRAFT_MODEL);
    }

    #[test]
    fn join_system_blocks_skips_empty_and_joins_in_order() {
        let blocks = vec![
            SystemBlock {
                text: "character".to_string(),
                cache_hint: CacheHint::Stable,
            },
            SystemBlock {
                text: "  ".to_string(),
                cache_hint: CacheHint::Session,
            },
            SystemBlock {
                text: "snapshot".to_string(),
                cache_hint: CacheHint::Volatile,
            },
        ];
        assert_eq!(join_system_blocks(&blocks), "character\n\nsnapshot");
    }

    #[test]
    fn model_request_preserves_single_user_prompt() {
        let request = ModelRequest::new(Tier::Conversation, "system", "revise this");
        assert_eq!(
            ModelRouter::request_user_text(&request).unwrap(),
            "revise this"
        );
    }

    #[test]
    fn model_request_labels_multi_turn_messages() {
        let request = ModelRequest {
            tier: Tier::Craft,
            system_blocks: vec![SystemBlock {
                text: "system".to_string(),
                cache_hint: CacheHint::Stable,
            }],
            messages: vec![
                ModelMessage::user("draft one"),
                ModelMessage::assistant("drafted"),
                ModelMessage::user("make it sharper"),
            ],
            json_schema: None,
            stream: false,
            max_tokens: None,
            temperature: 0.0,
            timeout_ms: None,
            purpose: Some("test".to_string()),
        };

        assert_eq!(
            ModelRouter::request_user_text(&request).unwrap(),
            "user: draft one\n\nassistant: drafted\n\nuser: make it sharper"
        );
    }

    #[test]
    fn model_request_options_map_json_object() {
        let request = ModelRequest::new(Tier::Reflex, "system", "classify").with_options(
            GenerateOptions {
                temperature: 0.2,
                max_tokens: Some(12),
                json_object: true,
                timeout_ms: Some(50),
            },
        );

        assert_eq!(request.temperature, 0.2);
        assert_eq!(request.max_tokens, Some(12));
        assert_eq!(request.timeout_ms, Some(50));
        assert!(request.json_schema.is_some());
    }

    #[test]
    fn tier_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Tier::Craft).unwrap(), "\"craft\"");
        assert_eq!(Tier::Judgment.as_str(), "judgment");
    }
}
