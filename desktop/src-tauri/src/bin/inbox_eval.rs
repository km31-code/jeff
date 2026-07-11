use std::{env, fs, path::PathBuf, process};

use anyhow::{anyhow, Context, Result};
use jeff_desktop::gmail_core::{triage_precision, EmailMessage, TriageContext};
use serde::Deserialize;

const DEFAULT_PRECISION_BAR: f32 = 0.80;

#[derive(Deserialize)]
struct InboxEval {
    context: TriageContext,
    messages: Vec<EmailMessage>,
    labels: Vec<bool>,
}

fn main() {
    match run() {
        Ok(true) => process::exit(0),
        Ok(false) => process::exit(1),
        Err(err) => {
            eprintln!("inbox eval error: {err:#}");
            process::exit(2);
        }
    }
}

fn run() -> Result<bool> {
    let path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("eval/inbox_eval.json"));
    let raw = fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let eval: InboxEval = serde_json::from_str(&raw).context("failed to parse inbox eval")?;
    if eval.messages.len() != eval.labels.len() {
        return Err(anyhow!(
            "messages ({}) and labels ({}) length mismatch",
            eval.messages.len(),
            eval.labels.len()
        ));
    }
    if eval.messages.len() < 50 {
        return Err(anyhow!(
            "triage eval requires at least 50 messages, got {}",
            eval.messages.len()
        ));
    }
    let (precision, true_positive, flagged) =
        triage_precision(&eval.messages, &eval.labels, &eval.context);
    println!(
        "triage precision {:.1}% ({}/{} flagged correct); bar {:.0}%",
        precision * 100.0,
        true_positive,
        flagged,
        DEFAULT_PRECISION_BAR * 100.0
    );
    Ok(precision >= DEFAULT_PRECISION_BAR)
}
