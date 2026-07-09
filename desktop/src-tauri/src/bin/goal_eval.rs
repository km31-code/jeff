// apex b2: goal extraction eval harness.
// scores the retired prefix matcher and the deterministic heuristic against the
// labeled eval set (always, no network), and — when JEFF_RUN_EXTERNAL_EVAL=1
// with an OpenAI key — the reflex-tier llm extractor, enforcing the >=85% bar.

use std::{env, fs, path::PathBuf, process};

use anyhow::{anyhow, Context, Result};
use jeff_desktop::goal_extraction::{self, GoalEvalCase};
use jeff_desktop::model_router::{ModelRouter, RouterConfig};

fn default_eval_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("eval/goal_extraction_eval.json")
}

fn load_cases(path: &PathBuf) -> Result<Vec<GoalEvalCase>> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read eval set at {}", path.display()))?;
    let cases: Vec<GoalEvalCase> =
        serde_json::from_str(&raw).context("failed to parse goal_extraction_eval.json")?;
    if cases.len() < 30 {
        return Err(anyhow!(
            "goal eval set must have at least 30 cases, found {}",
            cases.len()
        ));
    }
    Ok(cases)
}

fn score<F>(cases: &[GoalEvalCase], predict: F) -> (usize, usize)
where
    F: Fn(&GoalEvalCase) -> Option<String>,
{
    let correct = cases
        .iter()
        .filter(|case| goal_extraction::prediction_is_correct(case, &predict(case)))
        .count();
    (correct, cases.len())
}

fn pct(correct: usize, total: usize) -> f64 {
    if total == 0 {
        0.0
    } else {
        correct as f64 / total as f64 * 100.0
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("goal_eval error: {err:#}");
        process::exit(1);
    }
}

fn run() -> Result<()> {
    let path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_eval_path);
    let cases = load_cases(&path)?;

    let (prefix_correct, total) = score(&cases, |c| c.prefix_goal());
    let (heuristic_correct, _) = score(&cases, |c| c.heuristic_goal());

    println!("goal extraction eval ({total} cases)");
    println!(
        "  retired prefix matcher: {prefix_correct}/{total} ({:.1}%)",
        pct(prefix_correct, total)
    );
    println!(
        "  heuristic extractor:    {heuristic_correct}/{total} ({:.1}%)",
        pct(heuristic_correct, total)
    );

    let heuristic_pass_bar: f64 = env::var("JEFF_GOAL_HEURISTIC_EVAL_PASS_PCT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(85.0);

    // the deterministic contrast is the always-on gate: the heuristic fallback
    // must beat the retired matcher by a clear margin and clear the same
    // accuracy bar used for the live llm extractor.
    if pct(prefix_correct, total) >= 40.0 {
        return Err(anyhow!(
            "retired prefix matcher scored {:.1}% (expected < 40% on this set)",
            pct(prefix_correct, total)
        ));
    }
    if pct(heuristic_correct, total) < heuristic_pass_bar {
        return Err(anyhow!(
            "heuristic extractor scored {:.1}% (< {:.1}% pass bar)",
            pct(heuristic_correct, total),
            heuristic_pass_bar
        ));
    }
    if heuristic_correct <= prefix_correct + total / 5 {
        return Err(anyhow!(
            "heuristic did not beat the prefix matcher by a clear margin"
        ));
    }

    let run_llm = env::var("JEFF_RUN_EXTERNAL_EVAL").ok().as_deref() == Some("1");
    if !run_llm {
        println!("  llm extractor: SKIP (set JEFF_RUN_EXTERNAL_EVAL=1 with an OpenAI key)");
        return Ok(());
    }

    let pass_bar: f64 = env::var("JEFF_GOAL_EVAL_PASS_PCT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(85.0);

    let router = ModelRouter::new(RouterConfig::default());
    let mut llm_predictions = Vec::with_capacity(cases.len());
    for case in &cases {
        let extraction = goal_extraction::extract_goal(&router, &case.messages())
            .with_context(|| format!("llm extractor failed for case {}", case.id))?;
        llm_predictions.push(extraction.goal);
    }
    let llm_correct = cases
        .iter()
        .zip(llm_predictions.iter())
        .filter(|(case, prediction)| goal_extraction::prediction_is_correct(case, prediction))
        .count();
    if env::var("JEFF_GOAL_EVAL_VERBOSE").ok().as_deref() == Some("1") {
        for (case, prediction) in cases.iter().zip(llm_predictions.iter()) {
            if !goal_extraction::prediction_is_correct(case, prediction) {
                println!(
                    "  miss {} expected={:?} predicted={:?}",
                    case.id, case.expected_goal, prediction
                );
            }
        }
    }
    println!(
        "  llm extractor:          {llm_correct}/{total} ({:.1}%)",
        pct(llm_correct, total)
    );
    if pct(llm_correct, total) < pass_bar {
        return Err(anyhow!(
            "llm extractor scored {:.1}% (< {:.1}% pass bar)",
            pct(llm_correct, total),
            pass_bar
        ));
    }
    Ok(())
}
