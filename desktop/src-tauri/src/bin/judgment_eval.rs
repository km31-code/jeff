use std::{env, fs, path::PathBuf, process};

use anyhow::{anyhow, Context, Result};
use jeff_desktop::judgment_eval_core::{evaluate_stage2_fixture, JudgmentEvalScenario};

const DEFAULT_PASS_BAR: f32 = 0.85;

struct Args {
    scenarios_path: PathBuf,
    pass_bar: f32,
}

fn main() {
    match run() {
        Ok(true) => process::exit(0),
        Ok(false) => process::exit(1),
        Err(err) => {
            eprintln!("judgment eval error: {err:#}");
            process::exit(2);
        }
    }
}

fn run() -> Result<bool> {
    let args = parse_args()?;
    let scenarios = read_scenarios(&args.scenarios_path)?;
    if scenarios.len() < 20 {
        return Err(anyhow!(
            "judgment eval requires at least 20 scenarios, got {}",
            scenarios.len()
        ));
    }

    let mut passed = 0usize;
    for scenario in &scenarios {
        let actual = evaluate_stage2_fixture(scenario);
        let decision_ok = actual.decision == scenario.expected.decision;
        let channel_ok = scenario
            .expected
            .channels
            .iter()
            .any(|channel| channel == &actual.channel);
        if decision_ok && channel_ok {
            passed += 1;
            println!(
                "[PASS] {} {} -> {}/{}",
                scenario.id, scenario.category, actual.decision, actual.channel
            );
        } else {
            println!(
                "[FAIL] {} {} expected {}/{:?}, got {}/{} ({})",
                scenario.id,
                scenario.category,
                scenario.expected.decision,
                scenario.expected.channels,
                actual.decision,
                actual.channel,
                actual.reason
            );
        }
    }

    let agreement = passed as f32 / scenarios.len() as f32;
    println!(
        "{}/{} passed; agreement {:.1}% (pass bar {:.1}%)",
        passed,
        scenarios.len(),
        agreement * 100.0,
        args.pass_bar * 100.0
    );
    Ok(agreement >= args.pass_bar)
}

fn parse_args() -> Result<Args> {
    let mut raw = env::args().skip(1);
    let scenarios_path = raw
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("usage: judgment_eval <scenarios.json> [--pass-bar FRACTION]"))?;
    let mut pass_bar = DEFAULT_PASS_BAR;

    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--pass-bar" => {
                let value = raw
                    .next()
                    .ok_or_else(|| anyhow!("--pass-bar requires a value"))?;
                pass_bar = value.parse::<f32>().context("failed to parse --pass-bar")?;
                if !(0.0..=1.0).contains(&pass_bar) {
                    return Err(anyhow!("--pass-bar must be between 0 and 1"));
                }
            }
            other => return Err(anyhow!("unknown argument: {other}")),
        }
    }

    Ok(Args {
        scenarios_path,
        pass_bar,
    })
}

fn read_scenarios(path: &PathBuf) -> Result<Vec<JudgmentEvalScenario>> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).context("failed to parse judgment eval scenarios")
}
