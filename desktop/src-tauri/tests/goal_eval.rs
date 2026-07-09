// apex b2: hermetic goal-extraction contrast. asserts the heuristic extractor
// beats the retired prefix matcher by a clear margin on the labeled eval set,
// with no network. the >=85% llm gate lives in the goal_eval binary + script.

use std::{fs, path::PathBuf};

use jeff_desktop::goal_extraction::{prediction_is_correct, GoalEvalCase};

fn load_cases() -> Vec<GoalEvalCase> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("eval/goal_extraction_eval.json");
    let raw = fs::read_to_string(&path).expect("failed to read goal_extraction_eval.json");
    serde_json::from_str(&raw).expect("goal_extraction_eval.json must parse")
}

fn score<F: Fn(&GoalEvalCase) -> Option<String>>(cases: &[GoalEvalCase], predict: F) -> f64 {
    let correct = cases
        .iter()
        .filter(|c| prediction_is_correct(c, &predict(c)))
        .count();
    correct as f64 / cases.len() as f64
}

#[test]
fn b2_eval_set_has_min_cases_and_category_coverage() {
    let cases = load_cases();
    assert!(
        cases.len() >= 30,
        "expected >= 30 cases, got {}",
        cases.len()
    );
    for category in [
        "explicit",
        "paraphrase",
        "imperative",
        "implicit",
        "no_goal",
        "no_goal_trap",
    ] {
        assert!(
            cases.iter().any(|c| c.category == category),
            "eval set missing category {category}"
        );
    }
    let no_goal = cases.iter().filter(|c| c.expected_goal.is_none()).count();
    assert!(
        no_goal >= 4,
        "expected several no-goal controls, got {no_goal}"
    );
}

#[test]
fn b2_heuristic_beats_retired_prefix_matcher() {
    let cases = load_cases();
    let prefix = score(&cases, |c| c.prefix_goal());
    let heuristic = score(&cases, |c| c.heuristic_goal());

    // recorded for the contrast, per the b2 done criteria.
    println!(
        "goal extraction: prefix={:.1}% heuristic={:.1}%",
        prefix * 100.0,
        heuristic * 100.0
    );

    assert!(
        prefix < 0.40,
        "retired prefix matcher should score < 40%, got {:.1}%",
        prefix * 100.0
    );
    assert!(
        heuristic >= 0.85,
        "heuristic extractor should score >= 85%, got {:.1}%",
        heuristic * 100.0
    );
    assert!(
        heuristic - prefix >= 0.25,
        "heuristic should beat prefix by >= 25 points (prefix {:.1}%, heuristic {:.1}%)",
        prefix * 100.0,
        heuristic * 100.0
    );
}
