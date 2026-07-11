use std::{env, fs, path::PathBuf, process};

use anyhow::{anyhow, Context, Result};
use jeff_desktop::crisis_core::{evaluate_crisis_case, CrisisEvalCase};

fn main() {
    match run() {
        Ok(true) => process::exit(0),
        Ok(false) => process::exit(1),
        Err(err) => {
            eprintln!("crisis eval error: {err:#}");
            process::exit(2);
        }
    }
}

fn run() -> Result<bool> {
    let path = env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("usage: crisis_eval <cases.json>"))?;
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let cases: Vec<CrisisEvalCase> =
        serde_json::from_str(&raw).context("failed to parse crisis eval cases")?;
    if cases.len() < 8 {
        return Err(anyhow!("expected at least 8 crisis eval cases"));
    }

    let mut passed = 0usize;
    for case in &cases {
        let fired = evaluate_crisis_case(case);
        if fired == case.expected_fire {
            passed += 1;
            println!("[PASS] {} {:?} fire={}", case.id, case.class, fired);
        } else {
            println!(
                "[FAIL] {} {:?} expected fire={}, got {}",
                case.id, case.class, case.expected_fire, fired
            );
        }
    }
    println!("{}/{} passed", passed, cases.len());
    Ok(passed == cases.len())
}
