use std::{env, fs, path::PathBuf, process};

use anyhow::{anyhow, Context, Result};
use jeff_desktop::{
    agent_eval_core::{evaluate_agent_contract_in_workspace, AgentEvalContract},
    store::TaskStore,
};

// apex e2 unlocked the 5 web-research contracts (they now run against seeded
// web-source fixtures), so all 20 contracts are non-gated and must pass.
const REQUIRED_MIN_PASSES: usize = 17;
const MIN_NON_GATED: usize = 20;
const MIN_GATED: usize = 0;

struct Args {
    contracts_path: PathBuf,
    min_passes: usize,
}

fn main() {
    match run() {
        Ok(true) => process::exit(0),
        Ok(false) => process::exit(1),
        Err(err) => {
            eprintln!("agent eval error: {err:#}");
            process::exit(2);
        }
    }
}

fn run() -> Result<bool> {
    let args = parse_args()?;
    let contracts = read_contracts(&args.contracts_path)?;

    let gated = contracts.iter().filter(|c| c.gated).count();
    let non_gated = contracts.len() - gated;
    if non_gated < MIN_NON_GATED {
        return Err(anyhow!(
            "agent eval requires at least {MIN_NON_GATED} non-gated contracts, got {non_gated}"
        ));
    }
    if gated < MIN_GATED {
        return Err(anyhow!(
            "agent eval requires at least {MIN_GATED} e2-gated contracts, got {gated}"
        ));
    }
    if args.min_passes < REQUIRED_MIN_PASSES {
        return Err(anyhow!(
            "agent eval pass floor is immutable: requested {}, required at least {REQUIRED_MIN_PASSES}",
            args.min_passes
        ));
    }
    if args.min_passes > non_gated {
        return Err(anyhow!(
            "agent eval min passes {} exceeds {} non-gated contracts",
            args.min_passes,
            non_gated
        ));
    }

    let mut passed = 0usize;
    let mut ran = 0usize;
    for contract in &contracts {
        if contract.gated {
            println!(
                "[SKIP] {} ({}) e2-gated until web research lands",
                contract.id, contract.category
            );
            continue;
        }
        ran += 1;
        let dir = fresh_store_dir(&contract.id)?;
        let store = TaskStore::initialize(&dir)
            .with_context(|| format!("failed to init store for contract {}", contract.id))?;
        let eval_root = args
            .contracts_path
            .parent()
            .ok_or_else(|| anyhow!("contracts path has no parent"))?;
        let outcome = evaluate_agent_contract_in_workspace(&store, contract, eval_root)?;
        let _ = fs::remove_dir_all(&dir);
        if outcome.passed {
            passed += 1;
            println!("[PASS] {} ({})", contract.id, contract.category);
        } else {
            println!(
                "[FAIL] {} ({}) -> {}",
                contract.id, contract.category, outcome.reason
            );
        }
    }

    println!(
        "{}/{} non-gated contracts passed (min {}); {} e2-gated skipped",
        passed, ran, args.min_passes, gated
    );
    Ok(passed >= args.min_passes)
}

fn parse_args() -> Result<Args> {
    let mut raw = env::args().skip(1);
    let contracts_path = raw
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("usage: agent_eval <contracts.json> [--min-passes N]"))?;
    let mut min_passes = REQUIRED_MIN_PASSES;

    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--min-passes" => {
                let value = raw
                    .next()
                    .ok_or_else(|| anyhow!("--min-passes requires a value"))?;
                min_passes = value.parse::<usize>().context("failed to parse --min-passes")?;
            }
            other => return Err(anyhow!("unknown argument: {other}")),
        }
    }

    Ok(Args {
        contracts_path,
        min_passes,
    })
}

fn read_contracts(path: &PathBuf) -> Result<Vec<AgentEvalContract>> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).context("failed to parse agent eval contracts")
}

fn fresh_store_dir(contract_id: &str) -> Result<PathBuf> {
    let safe: String = contract_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let dir = env::temp_dir().join(format!("jeff_agent_eval_{}_{}", process::id(), safe));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).with_context(|| format!("failed to create {}", dir.display()))?;
    Ok(dir)
}
