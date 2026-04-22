// intent classifier eval harness (m14.2)
// gated on OPENAI_API_KEY — skips when not set so ci does not require credentials.
// run with: cargo test --manifest-path src-tauri/Cargo.toml --test intent_eval -- --nocapture

use jeff_desktop::classifier::classify_intent;
use serde::Deserialize;
use std::time::Instant;

#[derive(Debug, Deserialize)]
struct ExpectedSlots {
    target_description: Option<String>,
    instruction: Option<String>,
    draft_type: Option<String>,
    topic: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EvalExample {
    input: String,
    expected_intent: String,
    expected_slots: ExpectedSlots,
}

#[test]
fn intent_classifier_accuracy_and_latency() {
    let api_key = match std::env::var("OPENAI_API_KEY") {
        Ok(k) if !k.trim().is_empty() => k,
        _ => {
            println!("[intent_eval] OPENAI_API_KEY not set — skipping live eval");
            return;
        }
    };

    let fixture_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/intent_eval_set.json"
    );

    let raw = std::fs::read_to_string(fixture_path).expect("failed to read intent_eval_set.json");
    let examples: Vec<EvalExample> =
        serde_json::from_str(&raw).expect("failed to parse intent_eval_set.json");

    assert!(!examples.is_empty(), "eval set must not be empty");

    let mut correct = 0usize;
    let mut latencies_ms: Vec<u128> = Vec::new();
    let mut slot_checks = 0usize;
    let mut slot_correct = 0usize;

    for (i, ex) in examples.iter().enumerate() {
        let start = Instant::now();
        let result = classify_intent(&ex.input, &api_key)
            .unwrap_or_else(|e| panic!("classify_intent failed on example {i}: {e}"));
        let elapsed = start.elapsed().as_millis();
        latencies_ms.push(elapsed);

        let got = format!("{:?}", result.intent).to_lowercase();
        let expected = ex.expected_intent.to_lowercase();

        // intent enum debug format produces "Answer", "Revision", etc.
        // normalise both sides to lowercase for comparison.
        let got_norm = match got.as_str() {
            "answer" => "answer",
            "revision" => "revision",
            "subtask" => "subtask",
            "suggestion" => "suggestion",
            _ => "unknown",
        };

        if got_norm == expected.as_str() {
            correct += 1;
        } else {
            println!(
                "[intent_eval] WRONG example {i}: input={:?} expected={} got={}",
                ex.input, expected, got_norm
            );
        }

        let slot_pairs = [
            (
                "target_description",
                ex.expected_slots.target_description.as_deref(),
                result.slots.target_description.as_deref(),
            ),
            (
                "instruction",
                ex.expected_slots.instruction.as_deref(),
                result.slots.instruction.as_deref(),
            ),
            (
                "draft_type",
                ex.expected_slots.draft_type.as_deref(),
                result.slots.draft_type.as_deref(),
            ),
            (
                "topic",
                ex.expected_slots.topic.as_deref(),
                result.slots.topic.as_deref(),
            ),
        ];

        for (slot_name, expected_slot, actual_slot) in slot_pairs {
            let Some(expected_slot) = expected_slot else {
                continue;
            };
            slot_checks += 1;

            if slot_matches(expected_slot, actual_slot) {
                slot_correct += 1;
            } else {
                println!(
                    "[intent_eval] SLOT MISMATCH example {i}: slot={} expected={:?} got={:?}",
                    slot_name, expected_slot, actual_slot
                );
            }
        }
    }

    let total = examples.len();
    let accuracy = correct as f64 / total as f64;
    let slot_accuracy = if slot_checks == 0 {
        1.0
    } else {
        slot_correct as f64 / slot_checks as f64
    };

    latencies_ms.sort_unstable();
    let p50 = percentile_ms(&latencies_ms, 0.50);
    let p95 = percentile_ms(&latencies_ms, 0.95);

    println!(
        "[intent_eval] accuracy={}/{} ({:.1}%)  slot accuracy={}/{} ({:.1}%)  p50={}ms  p95={}ms",
        correct,
        total,
        accuracy * 100.0,
        slot_correct,
        slot_checks,
        slot_accuracy * 100.0,
        p50,
        p95
    );

    assert!(
        accuracy >= 0.90,
        "intent classifier accuracy {:.1}% is below the 90% threshold ({}/{} correct)",
        accuracy * 100.0,
        correct,
        total
    );

    assert!(
        slot_accuracy >= 0.75,
        "intent classifier slot accuracy {:.1}% is below the 75% threshold ({}/{} correct)",
        slot_accuracy * 100.0,
        slot_correct,
        slot_checks
    );

    assert!(
        p50 < 150,
        "intent classifier p50 latency {}ms exceeds the p50 < 150ms budget",
        p50
    );

    assert!(
        p95 < 450,
        "intent classifier p95 latency {}ms exceeds the p95 < 450ms budget",
        p95
    );
}

fn percentile_ms(values: &[u128], percentile: f64) -> u128 {
    if values.is_empty() {
        return 0;
    }
    let rank = ((values.len() as f64 - 1.0) * percentile).round() as usize;
    values[rank.min(values.len() - 1)]
}

fn slot_matches(expected: &str, actual: Option<&str>) -> bool {
    let Some(actual) = actual else {
        return false;
    };

    let expected_norm = normalize_slot(expected);
    let actual_norm = normalize_slot(actual);

    if expected_norm.is_empty() || actual_norm.is_empty() {
        return false;
    }

    expected_norm == actual_norm
        || expected_norm.contains(&actual_norm)
        || actual_norm.contains(&expected_norm)
}

fn normalize_slot(raw: &str) -> String {
    raw.to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
}
