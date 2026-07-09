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

use crate::local_runtime::LocalRuntime;
use crate::providers::anthropic::{self, AnthropicRequest};
use crate::providers::local::{classify_intent_locally, LocalReasoningProvider};
use crate::store::TaskStore;

pub const TIER_MODEL_MAP_SETTING: &str = "tier_model_map";

// default models per tier. apex a3 moves reflex to an on-device local provider;
// judgment and craft default to anthropic because the character spec is written
// for a model that can hold a voice.
pub const DEFAULT_REFLEX_MODEL: &str = crate::local_runtime::LOCAL_REASONING_MODEL_ID;
pub const DEFAULT_CONVERSATION_MODEL: &str = "claude-haiku-4-5";
pub const DEFAULT_JUDGMENT_MODEL: &str = "claude-sonnet-5";
pub const DEFAULT_CRAFT_MODEL: &str = "claude-sonnet-5";
// used when an anthropic tier must fall back because no anthropic key exists.
pub const OPENAI_FALLBACK_MODEL: &str = "gpt-4o-mini";

const CLASSIFY_TIMEOUT_OPENAI_MS: u64 = 300;
const CLASSIFY_TIMEOUT_ANTHROPIC_MS: u64 = 2000;
const CLASSIFY_TIMEOUT_OPENAI_ENV: &str = "JEFF_CLASSIFY_TIMEOUT_OPENAI_MS";

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

impl LlmUsage {
    pub fn cached_ratio(&self) -> f64 {
        if self.input_tokens == 0 {
            0.0
        } else {
            self.cached_tokens as f64 / self.input_tokens as f64
        }
    }
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
    pub budget_key: Option<String>,
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
            budget_key: None,
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

    #[allow(dead_code)]
    pub fn with_budget_key(mut self, budget_key: impl Into<String>) -> Self {
        self.budget_key = Some(budget_key.into());
        self
    }

    pub fn new_blocks(
        tier: Tier,
        system_blocks: Vec<SystemBlock>,
        user: impl Into<String>,
    ) -> Self {
        Self {
            tier,
            system_blocks,
            messages: vec![ModelMessage::user(user)],
            json_schema: None,
            stream: false,
            max_tokens: None,
            temperature: 0.0,
            timeout_ms: None,
            purpose: None,
            budget_key: None,
        }
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
    Local,
    OpenAi,
    Anthropic,
}

impl ProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderKind::Local => "local",
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
                provider: ProviderKind::Local,
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
    local_runtime: Option<Arc<LocalRuntime>>,
    store: Option<TaskStore>,
}

impl ModelRouter {
    #[allow(dead_code)]
    pub fn new(config: RouterConfig) -> Self {
        Self {
            config: RwLock::new(config),
            local_runtime: None,
            store: None,
        }
    }

    #[allow(dead_code)]
    pub fn with_local_runtime(config: RouterConfig, local_runtime: Arc<LocalRuntime>) -> Self {
        Self {
            config: RwLock::new(config),
            local_runtime: Some(local_runtime),
            store: None,
        }
    }

    pub fn with_local_runtime_and_store(
        config: RouterConfig,
        local_runtime: Arc<LocalRuntime>,
        store: TaskStore,
    ) -> Self {
        Self {
            config: RwLock::new(config),
            local_runtime: Some(local_runtime),
            store: Some(store),
        }
    }

    // loads persisted config from app_settings, falling back to defaults on
    // absence or parse failure. a bad stored config never blocks startup.
    #[allow(dead_code)]
    pub fn from_store(store: &TaskStore) -> Self {
        Self::from_store_with_local_runtime(store, None)
    }

    pub fn from_store_with_local_runtime(
        store: &TaskStore,
        local_runtime: Option<Arc<LocalRuntime>>,
    ) -> Self {
        let config = match store.get_app_setting(TIER_MODEL_MAP_SETTING) {
            Ok(Some(raw)) => match RouterConfig::parse(&raw) {
                Ok(parsed) => parsed,
                Err(err) => {
                    eprintln!("[jeff] model_router_config_invalid: {err}; using defaults");
                    RouterConfig::default()
                }
            },
            _ => RouterConfig::default(),
        };
        match local_runtime {
            Some(local_runtime) => Self::with_local_runtime_and_store(config, local_runtime, store.clone()),
            None => Self {
                config: RwLock::new(config),
                local_runtime: None,
                store: Some(store.clone()),
            },
        }
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
        self.local_runtime.is_some()
            || crate::secrets::resolve_openai_api_key().api_key.is_some()
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
            ProviderKind::Local => configured,
            ProviderKind::OpenAi => configured,
        }
    }

    fn log_usage(&self, tier: Tier, cfg: &TierConfig, usage: &LlmUsage, purpose: &str) {
        self.log_usage_for_budget_key(
            crate::cost_governor::budget_key_for_tier(tier),
            cfg,
            usage,
            purpose,
        );
    }

    fn log_usage_for_budget_key(
        &self,
        budget_key: &str,
        cfg: &TierConfig,
        usage: &LlmUsage,
        purpose: &str,
    ) {
        let cumulative = crate::latency::record_llm_usage(*usage);
        let est_cost_usd = crate::cost_governor::record_usage_for_budget_key(
            self.store.as_ref(),
            budget_key,
            cfg.provider,
            &cfg.model,
            purpose,
            *usage,
        )
        .unwrap_or_else(|err| {
            eprintln!("[jeff] llm_usage_log_failed: {err}");
            crate::cost_governor::estimate_cost_usd(cfg.provider, &cfg.model, *usage)
        });
        eprintln!(
            "[jeff] llm_usage tier={} provider={} model={} purpose={} input={} output={} cached={} est_cost_usd={:.6} cached_ratio={:.3} cumulative_cached_ratio={:.3}",
            budget_key,
            cfg.provider.as_str(),
            cfg.model,
            crate::cost_governor::normalize_purpose(purpose),
            usage.input_tokens,
            usage.output_tokens,
            usage.cached_tokens,
            est_cost_usd,
            usage.cached_ratio(),
            cumulative.cached_ratio
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

    fn generate_blocking_with_config(
        cfg: &TierConfig,
        request: &ModelRequest,
        system: &str,
        user: &str,
        json_object: bool,
    ) -> Result<(String, LlmUsage)> {
        match cfg.provider {
            ProviderKind::OpenAi => crate::providers::openai_generate_blocking(
                &cfg.model,
                system,
                user,
                request.temperature,
                request.max_tokens,
                json_object,
                request.timeout_ms,
            ),
            ProviderKind::Anthropic => anthropic::generate_blocking(&AnthropicRequest {
                model: &cfg.model,
                system_blocks: &request.system_blocks,
                user,
                temperature: request.temperature,
                max_tokens: request.max_tokens,
                json_only: json_object,
                timeout_ms: request.timeout_ms,
            }),
            ProviderKind::Local => {
                Err(anyhow!("local provider cannot be used as a cloud fallback"))
            }
        }
    }

    fn local_reasoning_provider(&self, model: &str) -> Result<LocalReasoningProvider> {
        let runtime = self
            .local_runtime
            .clone()
            .ok_or_else(|| anyhow!("local runtime is not configured"))?;
        Ok(LocalReasoningProvider::new(runtime, model.to_string()))
    }

    fn local_cloud_fallback(&self) -> Option<TierConfig> {
        if crate::secrets::resolve_openai_api_key().api_key.is_some() {
            Some(TierConfig {
                provider: ProviderKind::OpenAi,
                model: OPENAI_FALLBACK_MODEL.to_string(),
            })
        } else if crate::secrets::resolve_anthropic_api_key().is_some() {
            Some(TierConfig {
                provider: ProviderKind::Anthropic,
                model: DEFAULT_JUDGMENT_MODEL.to_string(),
            })
        } else {
            None
        }
    }

    pub fn route(&self, mut request: ModelRequest) -> Result<ModelResponse> {
        if request.stream {
            return Err(anyhow!("streaming model requests must use route_streaming"));
        }

        let purpose = request
            .purpose
            .clone()
            .unwrap_or_else(|| "default".to_string());
        let budget_key = request
            .budget_key
            .clone()
            .unwrap_or_else(|| crate::cost_governor::budget_key_for_tier(request.tier).to_string());
        let budget_decision = crate::cost_governor::preflight_for_budget_key(
            self.store.as_ref(),
            request.tier,
            &budget_key,
            &purpose,
        );
        if budget_decision.degraded {
            eprintln!(
                "[jeff] model_router_budget_degraded from={} to={} purpose={} notice={}",
                budget_decision.requested_tier.as_str(),
                budget_decision.effective_tier.as_str(),
                crate::cost_governor::normalize_purpose(&purpose),
                budget_decision
                    .notice
                    .as_deref()
                    .unwrap_or("<already-sent>")
            );
            request.tier = budget_decision.effective_tier;
        }

        let system = join_system_blocks(&request.system_blocks);
        let user = Self::request_user_text(&request)?;
        let cfg = self.resolve(request.tier);
        let json_object = request.json_schema.is_some();
        let (effective_cfg, text, usage) = match cfg.provider {
            ProviderKind::OpenAi | ProviderKind::Anthropic => {
                let (text, usage) = Self::generate_blocking_with_config(
                    &cfg,
                    &request,
                    &system,
                    &user,
                    json_object,
                )?;
                (cfg.clone(), text, usage)
            }
            ProviderKind::Local => {
                let provider = self.local_reasoning_provider(&cfg.model);
                match provider.and_then(|provider| {
                    provider.generate_with_usage(
                        &system,
                        &user,
                        request.temperature,
                        request.max_tokens,
                        json_object,
                    )
                }) {
                    Ok((text, usage)) => (cfg.clone(), text, usage),
                    Err(err) => {
                        let fallback = self.local_cloud_fallback().ok_or_else(|| {
                            anyhow!(
                                "local provider failed and no cloud fallback key is configured: {err}"
                            )
                        })?;
                        eprintln!(
                            "[jeff] model_router_fallback tier={} reason=local_unavailable provider={} model={} error={}",
                            request.tier.as_str(),
                            fallback.provider.as_str(),
                            fallback.model,
                            err
                        );
                        let (text, usage) = Self::generate_blocking_with_config(
                            &fallback,
                            &request,
                            &system,
                            &user,
                            json_object,
                        )?;
                        (fallback, text, usage)
                    }
                }
            }
        };
        self.log_usage_for_budget_key(&budget_key, &effective_cfg, &usage, &purpose);
        Ok(ModelResponse {
            text,
            usage,
            provider: effective_cfg.provider,
            model: effective_cfg.model,
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
        let user = Self::request_user_text(&request)?;
        self.stream_blocks(request.tier, request.system_blocks, &user, cancel)
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

    pub fn generate_blocks(
        &self,
        tier: Tier,
        system_blocks: Vec<SystemBlock>,
        user: &str,
        options: GenerateOptions,
    ) -> Result<String> {
        Ok(self
            .route(ModelRequest::new_blocks(tier, system_blocks, user).with_options(options))?
            .text)
    }

    #[allow(dead_code)]
    pub async fn generate_async(
        &self,
        tier: Tier,
        system: &str,
        user: &str,
        options: GenerateOptions,
    ) -> Result<String> {
        self.generate_blocks_async(
            tier,
            vec![SystemBlock {
                text: system.to_string(),
                cache_hint: CacheHint::Volatile,
            }],
            user,
            options,
        )
        .await
    }

    pub async fn generate_blocks_async(
        &self,
        tier: Tier,
        system_blocks: Vec<SystemBlock>,
        user: &str,
        options: GenerateOptions,
    ) -> Result<String> {
        let purpose = "async_generation";
        let budget_decision = crate::cost_governor::preflight(self.store.as_ref(), tier, purpose);
        if budget_decision.degraded {
            eprintln!(
                "[jeff] model_router_budget_degraded from={} to={} purpose={} notice={}",
                budget_decision.requested_tier.as_str(),
                budget_decision.effective_tier.as_str(),
                purpose,
                budget_decision.notice.as_deref().unwrap_or("<already-sent>")
            );
        }
        let effective_tier = budget_decision.effective_tier;
        let cfg = self.resolve(effective_tier);
        let (text, usage) = match cfg.provider {
            ProviderKind::Local => {
                let system = join_system_blocks(&system_blocks);
                self.local_reasoning_provider(&cfg.model)?
                    .generate_with_usage(
                        &system,
                        user,
                        options.temperature,
                        options.max_tokens,
                        options.json_object,
                    )?
            }
            ProviderKind::OpenAi => {
                let system = join_system_blocks(&system_blocks);
                crate::providers::openai_generate_async(
                    &cfg.model,
                    &system,
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
                    system_blocks: &system_blocks,
                    user,
                    temperature: options.temperature,
                    max_tokens: options.max_tokens,
                    json_only: options.json_object,
                    timeout_ms: options.timeout_ms,
                })
                .await?
            }
        };
        self.log_usage(effective_tier, &cfg, &usage, purpose);
        Ok(text)
    }

    // streaming generation with the same channel contract as the legacy
    // openai streaming provider: text deltas until the channel closes.
    #[allow(dead_code)]
    pub fn stream(
        &self,
        tier: Tier,
        system: &str,
        user: &str,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<Result<String>>> {
        self.stream_blocks(
            tier,
            vec![SystemBlock {
                text: system.to_string(),
                cache_hint: CacheHint::Volatile,
            }],
            user,
            cancel,
        )
    }

    pub fn stream_blocks(
        &self,
        tier: Tier,
        system_blocks: Vec<SystemBlock>,
        user: &str,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<mpsc::Receiver<Result<String>>> {
        let purpose = "streaming";
        let budget_decision = crate::cost_governor::preflight(self.store.as_ref(), tier, purpose);
        if budget_decision.degraded {
            eprintln!(
                "[jeff] model_router_budget_degraded from={} to={} purpose={} notice={}",
                budget_decision.requested_tier.as_str(),
                budget_decision.effective_tier.as_str(),
                purpose,
                budget_decision.notice.as_deref().unwrap_or("<already-sent>")
            );
        }
        let effective_tier = budget_decision.effective_tier;
        let cfg = self.resolve(effective_tier);
        eprintln!(
            "[jeff] llm_stream_open tier={} provider={} model={}",
            effective_tier.as_str(),
            cfg.provider.as_str(),
            cfg.model
        );
        let system = join_system_blocks(&system_blocks);
        let receiver = match cfg.provider {
            ProviderKind::Local => Err(anyhow!("local provider does not support streaming")),
            ProviderKind::OpenAi => {
                crate::reasoning::OpenAiStreamingReasoningProvider::with_model(cfg.model.clone())
                    .stream_response(&system, user, cancel)
            }
            ProviderKind::Anthropic => {
                anthropic::stream(cfg.model.clone(), system_blocks, user.to_string(), cancel)
            }
        }?;
        self.log_usage(effective_tier, &cfg, &LlmUsage::default(), purpose);
        Ok(receiver)
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
        if cfg.provider == ProviderKind::Local {
            let started = std::time::Instant::now();
            if let Ok(provider) = self.local_reasoning_provider(&cfg.model) {
                match provider.generate_with_usage(
                    crate::classifier::SYSTEM_PROMPT,
                    trimmed,
                    0.0,
                    Some(300),
                    true,
                ) {
                    Ok((raw, usage)) => {
                        self.log_usage(Tier::Reflex, &cfg, &usage, "intent_classification");
                        let parsed = crate::classifier::parse_classification(&raw)?;
                        eprintln!(
                            "[jeff] local_reflex_classify mode=local elapsed_ms={} intent={:?}",
                            started.elapsed().as_millis(),
                            parsed.intent
                        );
                        return Ok(parsed);
                    }
                    Err(err) => {
                        if let Some(fallback) = self.local_cloud_fallback() {
                            eprintln!(
                                "[jeff] model_router_fallback tier=reflex reason=local_unavailable provider={} model={} error={}",
                                fallback.provider.as_str(),
                                fallback.model,
                                err
                            );
                            let raw = self.classify_with_cloud_config(trimmed, &fallback)?;
                            return crate::classifier::parse_classification(&raw);
                        }
                        eprintln!(
                            "[jeff] local_reflex_fallback mode=deterministic reason={} elapsed_ms={}",
                            err,
                            started.elapsed().as_millis()
                        );
                    }
                }
            }
            let parsed = classify_intent_locally(trimmed);
            eprintln!(
                "[jeff] local_reflex_classify mode=deterministic elapsed_ms={} intent={:?}",
                started.elapsed().as_millis(),
                parsed.intent
            );
            return Ok(parsed);
        }
        let raw = self.classify_with_cloud_config(trimmed, &cfg)?;
        crate::classifier::parse_classification(&raw)
    }

    fn classify_with_cloud_config(&self, trimmed: &str, cfg: &TierConfig) -> Result<String> {
        let timeout_ms = match cfg.provider {
            ProviderKind::OpenAi => timeout_override_ms(
                std::env::var(CLASSIFY_TIMEOUT_OPENAI_ENV).ok().as_deref(),
                CLASSIFY_TIMEOUT_OPENAI_MS,
            ),
            ProviderKind::Anthropic => CLASSIFY_TIMEOUT_ANTHROPIC_MS,
            ProviderKind::Local => CLASSIFY_TIMEOUT_OPENAI_MS,
        };
        let request = ModelRequest::new(Tier::Reflex, crate::classifier::SYSTEM_PROMPT, trimmed)
            .with_options(GenerateOptions {
                temperature: 0.0,
                max_tokens: Some(300),
                json_object: true,
                timeout_ms: Some(timeout_ms),
            });
        let system = join_system_blocks(&request.system_blocks);
        let user = Self::request_user_text(&request)?;
        let (raw, usage) =
            Self::generate_blocking_with_config(cfg, &request, &system, &user, true)?;
        self.log_usage(Tier::Reflex, cfg, &usage, "intent_classification");
        Ok(raw)
    }

    // returns a tier-bound handle implementing the legacy reasoning trait so
    // existing function signatures (and their tests) keep working unchanged.
    pub fn handle(
        self: &Arc<Self>,
        tier: Tier,
    ) -> Arc<dyn crate::providers::ReasoningModelProvider> {
        Arc::new(TierHandle {
            router: Arc::clone(self),
            tier,
        })
    }
}

fn timeout_override_ms(raw: Option<&str>, default: u64) -> u64 {
    raw.and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

struct TierHandle {
    router: Arc<ModelRouter>,
    tier: Tier,
}

impl crate::providers::ReasoningModelProvider for TierHandle {
    fn generate_response(&self, system_prompt: &str, user_prompt: &str) -> Result<String> {
        self.router.generate(self.tier, system_prompt, user_prompt)
    }

    fn generate_response_blocks(
        &self,
        system_blocks: &[SystemBlock],
        user_prompt: &str,
    ) -> Result<String> {
        self.router.generate_blocks(
            self.tier,
            system_blocks.to_vec(),
            user_prompt,
            GenerateOptions::default(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_spec() {
        let config = RouterConfig::default();
        assert_eq!(config.reflex.provider, ProviderKind::Local);
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
    fn a3_default_reflex_prefers_local_provider() {
        let config = RouterConfig::default();
        assert_eq!(config.reflex.provider.as_str(), "local");
        assert_eq!(
            config.reflex.model,
            crate::local_runtime::LOCAL_REASONING_MODEL_ID
        );
    }

    #[test]
    fn a3_classify_without_api_keys_uses_local_reflex() {
        let router = ModelRouter::new(RouterConfig::default());
        let result = router
            .classify("rewrite this introduction so it is shorter")
            .unwrap();
        assert_eq!(result.intent, crate::models::IntentLabel::Revision);
        assert!(result.confidence > 0.75);
    }

    #[test]
    fn a3_router_can_share_local_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Arc::new(crate::local_runtime::LocalRuntime::new(dir.path()));
        let router = ModelRouter::with_local_runtime(RouterConfig::default(), runtime.clone());
        assert!(router.any_key_available());
        assert!(runtime.status().deterministic_fallback_enabled);
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
            budget_key: None,
        };

        assert_eq!(
            ModelRouter::request_user_text(&request).unwrap(),
            "user: draft one\n\nassistant: drafted\n\nuser: make it sharper"
        );
    }

    #[test]
    fn model_request_options_map_json_object() {
        let request =
            ModelRequest::new(Tier::Reflex, "system", "classify").with_options(GenerateOptions {
                temperature: 0.2,
                max_tokens: Some(12),
                json_object: true,
                timeout_ms: Some(50),
            });

        assert_eq!(request.temperature, 0.2);
        assert_eq!(request.max_tokens, Some(12));
        assert_eq!(request.timeout_ms, Some(50));
        assert!(request.json_schema.is_some());
    }

    #[test]
    fn timeout_override_ignores_invalid_values() {
        assert_eq!(timeout_override_ms(None, 300), 300);
        assert_eq!(timeout_override_ms(Some(""), 300), 300);
        assert_eq!(timeout_override_ms(Some("0"), 300), 300);
        assert_eq!(timeout_override_ms(Some("5000"), 300), 5000);
    }

    #[test]
    fn tier_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Tier::Craft).unwrap(), "\"craft\"");
        assert_eq!(Tier::Judgment.as_str(), "judgment");
    }
}
