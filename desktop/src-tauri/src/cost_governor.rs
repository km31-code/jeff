use anyhow::{anyhow, Result};

use crate::{
    model_router::{LlmUsage, ProviderKind, Tier},
    models::{CostGovernorStatusDto, CostHistoryEntryDto, CostTierSpendDto},
    store::{LlmUsageLogInput, TaskStore},
};

pub const DEFAULT_REFLEX_DAILY_BUDGET_USD: f64 = 2.0;
pub const DEFAULT_CONVERSATION_DAILY_BUDGET_USD: f64 = 5.0;
pub const DEFAULT_JUDGMENT_DAILY_BUDGET_USD: f64 = 10.0;
pub const DEFAULT_CRAFT_DAILY_BUDGET_USD: f64 = 20.0;
pub const SPECULATION_BUDGET_KEY: &str = "speculation";
pub const CONSOLIDATION_BUDGET_KEY: &str = "consolidation";
pub const WORK_UNDERSTANDING_BUDGET_KEY: &str = "work_understanding";
pub const LATEST_NOTICE_KEY: &str = "llm_cost_notice_latest";

#[derive(Debug, Clone, PartialEq)]
pub struct BudgetDecision {
    pub requested_tier: Tier,
    pub effective_tier: Tier,
    pub degraded: bool,
    pub notice: Option<String>,
}

pub fn budget_key_for_tier(tier: Tier) -> &'static str {
    tier.as_str()
}

pub fn budget_setting_key(budget_key: &str) -> String {
    format!("llm_daily_budget:{budget_key}")
}

pub fn default_daily_budget_usd(budget_key: &str) -> f64 {
    match budget_key {
        "reflex" => DEFAULT_REFLEX_DAILY_BUDGET_USD,
        "conversation" => DEFAULT_CONVERSATION_DAILY_BUDGET_USD,
        "judgment" => DEFAULT_JUDGMENT_DAILY_BUDGET_USD,
        "craft" => DEFAULT_CRAFT_DAILY_BUDGET_USD,
        SPECULATION_BUDGET_KEY => 3.0,
        CONSOLIDATION_BUDGET_KEY => 3.0,
        WORK_UNDERSTANDING_BUDGET_KEY => 3.0,
        _ => DEFAULT_CONVERSATION_DAILY_BUDGET_USD,
    }
}

pub fn get_daily_budget_usd(store: &TaskStore, budget_key: &str) -> Result<f64> {
    let key = budget_setting_key(budget_key);
    Ok(store
        .get_app_setting(&key)?
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or_else(|| default_daily_budget_usd(budget_key)))
}

pub fn set_daily_budget_usd(store: &TaskStore, budget_key: &str, value: f64) -> Result<()> {
    if !value.is_finite() || value < 0.0 {
        return Err(anyhow!("daily budget must be a non-negative finite number"));
    }
    store.set_app_setting(&budget_setting_key(budget_key), &format!("{value:.6}"))
}

pub fn preflight(store: Option<&TaskStore>, requested_tier: Tier, purpose: &str) -> BudgetDecision {
    preflight_for_budget_key(
        store,
        requested_tier,
        budget_key_for_tier(requested_tier),
        purpose,
    )
}

pub fn preflight_for_budget_key(
    store: Option<&TaskStore>,
    requested_tier: Tier,
    budget_key: &str,
    purpose: &str,
) -> BudgetDecision {
    let Some(store) = store else {
        return BudgetDecision {
            requested_tier,
            effective_tier: requested_tier,
            degraded: false,
            notice: None,
        };
    };

    let mut effective = requested_tier;
    let uses_named_budget = budget_key != budget_key_for_tier(requested_tier);
    for candidate in degradation_chain(requested_tier) {
        let key = if uses_named_budget {
            budget_key
        } else {
            budget_key_for_tier(candidate)
        };
        let spent = store.sum_llm_usage_today(Some(key)).unwrap_or(0.0);
        let budget =
            get_daily_budget_usd(store, key).unwrap_or_else(|_| default_daily_budget_usd(key));
        if spent <= budget || matches!(candidate, Tier::Conversation | Tier::Reflex) {
            effective = candidate;
            break;
        }
    }

    let degraded = effective != requested_tier;
    let notice = degraded
        .then(|| {
            mark_degradation_notice(store, requested_tier, effective, purpose)
                .ok()
                .flatten()
        })
        .flatten();

    BudgetDecision {
        requested_tier,
        effective_tier: effective,
        degraded,
        notice,
    }
}

#[allow(dead_code)]
pub fn record_usage(
    store: Option<&TaskStore>,
    tier: Tier,
    provider: ProviderKind,
    model: &str,
    purpose: &str,
    usage: LlmUsage,
) -> Result<f64> {
    record_usage_for_budget_key(
        store,
        budget_key_for_tier(tier),
        provider,
        model,
        purpose,
        usage,
    )
}

pub fn record_usage_for_budget_key(
    store: Option<&TaskStore>,
    budget_key: &str,
    provider: ProviderKind,
    model: &str,
    purpose: &str,
    usage: LlmUsage,
) -> Result<f64> {
    let Some(store) = store else {
        return Ok(estimate_cost_usd(provider, model, usage));
    };
    let est_cost_usd = estimate_cost_usd(provider, model, usage);
    store.append_llm_usage_log(&LlmUsageLogInput {
        tier: budget_key.to_string(),
        model: model.to_string(),
        purpose: normalize_purpose(purpose).to_string(),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cached_tokens: usage.cached_tokens,
        est_cost_usd,
    })?;
    Ok(est_cost_usd)
}

pub fn status(store: &TaskStore) -> Result<CostGovernorStatusDto> {
    let today_total_usd = store.sum_llm_usage_today(None)?;
    let by_tier = store.sum_llm_usage_today_by_tier()?;
    let spent_for = |tier: &str| -> f64 {
        by_tier
            .iter()
            .find(|row| row.tier == tier)
            .map(|row| row.est_cost_usd)
            .unwrap_or(0.0)
    };

    let mut tiers = [
        Tier::Reflex,
        Tier::Conversation,
        Tier::Judgment,
        Tier::Craft,
    ]
    .into_iter()
    .map(|tier| {
        let key = budget_key_for_tier(tier);
        let budget =
            get_daily_budget_usd(store, key).unwrap_or_else(|_| default_daily_budget_usd(key));
        let spent = spent_for(key);
        CostTierSpendDto {
            tier: key.to_string(),
            budget_key: key.to_string(),
            budget_usd: budget,
            spent_usd: spent,
            over_budget: spent > budget,
            degrade_to: degrade_target(tier).map(|target| target.as_str().to_string()),
        }
    })
    .collect::<Vec<_>>();

    for key in [
        SPECULATION_BUDGET_KEY,
        CONSOLIDATION_BUDGET_KEY,
        WORK_UNDERSTANDING_BUDGET_KEY,
    ] {
        let budget =
            get_daily_budget_usd(store, key).unwrap_or_else(|_| default_daily_budget_usd(key));
        let spent = spent_for(key);
        tiers.push(CostTierSpendDto {
            tier: key.to_string(),
            budget_key: key.to_string(),
            budget_usd: budget,
            spent_usd: spent,
            over_budget: spent > budget,
            degrade_to: None,
        });
    }

    let history = store
        .llm_usage_history(7)?
        .into_iter()
        .map(|row| CostHistoryEntryDto {
            date: row.date,
            total_usd: row.est_cost_usd,
        })
        .collect();

    Ok(CostGovernorStatusDto {
        today_total_usd,
        tiers,
        history,
        last_notice: store.get_app_setting(LATEST_NOTICE_KEY)?,
    })
}

pub fn estimate_cost_usd(provider: ProviderKind, model: &str, usage: LlmUsage) -> f64 {
    if provider == ProviderKind::Local {
        return 0.0;
    }
    let billable_input = usage.input_tokens.saturating_sub(usage.cached_tokens) as f64;
    let cached_input = usage.cached_tokens as f64;
    let output = usage.output_tokens as f64;
    let model_lower = model.to_ascii_lowercase();
    let (input_per_million, cached_per_million, output_per_million) =
        if model_lower.contains("sonnet") {
            (3.0, 0.30, 15.0)
        } else if model_lower.contains("haiku") {
            (0.80, 0.08, 4.0)
        } else if model_lower.contains(crate::model_router::OPENAI_FALLBACK_MODEL) {
            (0.15, 0.075, 0.60)
        } else {
            (1.0, 0.10, 3.0)
        };
    ((billable_input * input_per_million)
        + (cached_input * cached_per_million)
        + (output * output_per_million))
        / 1_000_000.0
}

pub fn normalize_purpose(purpose: &str) -> &str {
    let trimmed = purpose.trim();
    if trimmed.is_empty() {
        "default"
    } else {
        trimmed
    }
}

fn degradation_chain(tier: Tier) -> Vec<Tier> {
    match tier {
        Tier::Craft => vec![Tier::Craft, Tier::Judgment, Tier::Conversation],
        Tier::Judgment => vec![Tier::Judgment, Tier::Conversation],
        Tier::Conversation => vec![Tier::Conversation],
        Tier::Reflex => vec![Tier::Reflex],
    }
}

fn degrade_target(tier: Tier) -> Option<Tier> {
    match tier {
        Tier::Craft => Some(Tier::Judgment),
        Tier::Judgment => Some(Tier::Conversation),
        Tier::Conversation | Tier::Reflex => None,
    }
}

fn mark_degradation_notice(
    store: &TaskStore,
    requested: Tier,
    effective: Tier,
    purpose: &str,
) -> Result<Option<String>> {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let key = format!(
        "llm_cost_notice_sent:{today}:{}:{}",
        requested.as_str(),
        effective.as_str()
    );
    if store.get_app_setting(&key)?.is_some() {
        return Ok(None);
    }
    let message = format!(
        "Cost governor moved {} work to {} for today ({})",
        requested.as_str(),
        effective.as_str(),
        normalize_purpose(purpose)
    );
    store.set_app_setting(&key, "1")?;
    store.set_app_setting(LATEST_NOTICE_KEY, &message)?;
    Ok(Some(message))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> (tempfile::TempDir, TaskStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn a4_forced_low_budget_degrades_craft_to_judgment() {
        let (_dir, store) = test_store();
        set_daily_budget_usd(&store, "craft", 0.0).unwrap();
        store
            .append_llm_usage_log(&LlmUsageLogInput {
                tier: "craft".to_string(),
                model: "test".to_string(),
                purpose: "test".to_string(),
                input_tokens: 1,
                output_tokens: 1,
                cached_tokens: 0,
                est_cost_usd: 0.01,
            })
            .unwrap();

        let decision = preflight(Some(&store), Tier::Craft, "test");
        assert_eq!(decision.effective_tier, Tier::Judgment);
        assert!(decision.degraded);
        assert!(decision.notice.is_some());
        assert!(status(&store).unwrap().last_notice.is_some());
    }

    #[test]
    fn a4_spend_status_total_equals_usage_log_sum() {
        let (_dir, store) = test_store();
        for tier in ["craft", "judgment"] {
            store
                .append_llm_usage_log(&LlmUsageLogInput {
                    tier: tier.to_string(),
                    model: "test".to_string(),
                    purpose: "test".to_string(),
                    input_tokens: 1,
                    output_tokens: 1,
                    cached_tokens: 0,
                    est_cost_usd: 0.25,
                })
                .unwrap();
        }
        let status = status(&store).unwrap();
        assert!((status.today_total_usd - 0.5).abs() < 0.0001);
        let tier_sum = status.tiers.iter().map(|tier| tier.spent_usd).sum::<f64>();
        assert!((tier_sum - status.today_total_usd).abs() < 0.0001);
    }

    #[test]
    fn a4_runaway_loop_simulation_trips_budget_and_degrades() {
        let (_dir, store) = test_store();
        set_daily_budget_usd(&store, "craft", 0.10).unwrap();
        for _ in 0..50 {
            store
                .append_llm_usage_log(&LlmUsageLogInput {
                    tier: "craft".to_string(),
                    model: "test".to_string(),
                    purpose: "runaway".to_string(),
                    input_tokens: 1_000,
                    output_tokens: 1_000,
                    cached_tokens: 0,
                    est_cost_usd: 0.01,
                })
                .unwrap();
        }
        let decision = preflight(Some(&store), Tier::Craft, "runaway");
        assert_eq!(decision.effective_tier, Tier::Judgment);
        assert!(status(&store)
            .unwrap()
            .tiers
            .iter()
            .any(|tier| { tier.tier == "craft" && tier.over_budget }));
    }
}
