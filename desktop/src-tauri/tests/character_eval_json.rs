use std::{collections::HashMap, fs, path::PathBuf};

use serde_json::Value;

const VIOLATION_TYPES: [&str; 8] = [
    "FillerPhrase",
    "PermissionSeeking",
    "DisagreementAsQuestion",
    "TrailingSummary",
    "ResultWithoutAssessment",
    "ExcessiveHedge",
    "NonAnswer",
    "SelfNarration",
];

#[test]
fn character_eval_json_has_minimum_cases() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = root.join("eval/character_eval.json");
    let raw = fs::read_to_string(&path).expect("failed to read character_eval.json");
    let cases: Vec<Value> = serde_json::from_str(&raw).expect("character_eval.json must parse");

    assert!(
        cases.len() >= 30,
        "expected at least 30 character eval cases, got {}",
        cases.len()
    );

    let mut clean_count = 0usize;
    let mut violation_counts = HashMap::<String, usize>::new();

    for case in &cases {
        assert!(
            case.get("id").and_then(Value::as_str).is_some(),
            "case missing id"
        );
        assert!(
            case.get("context").and_then(Value::as_str).is_some(),
            "case missing context"
        );
        assert!(
            case.get("input").and_then(Value::as_str).is_some(),
            "case missing input"
        );
        assert!(
            case.get("jeff_output").and_then(Value::as_str).is_some(),
            "case missing jeff_output"
        );
        let violations = case
            .get("violations")
            .and_then(Value::as_array)
            .expect("case violations must be an array");
        if violations.is_empty() {
            clean_count += 1;
        }
        for violation in violations {
            let name = violation
                .as_str()
                .expect("violation entries must be strings");
            assert!(
                VIOLATION_TYPES.contains(&name),
                "unknown violation type {name}"
            );
            *violation_counts.entry(name.to_string()).or_default() += 1;
        }
    }

    assert!(
        clean_count >= 18,
        "expected at least 18 clean cases, got {clean_count}"
    );
    for violation_type in VIOLATION_TYPES {
        let count = violation_counts.get(violation_type).copied().unwrap_or(0);
        assert!(
            count >= 2,
            "expected at least 2 cases for {violation_type}, got {count}"
        );
    }
}
