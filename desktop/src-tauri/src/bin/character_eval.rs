use std::{env, fs, path::PathBuf, process};

use anyhow::{anyhow, Context, Result};
use jeff_desktop::model_router::{
    GenerateOptions, ModelRequest, ModelRouter, ProviderKind, RouterConfig, Tier, TierConfig,
    OPENAI_FALLBACK_MODEL,
};
use serde::Deserialize;

const DEFAULT_GRADER_SYSTEM_PROMPT: &str = r#"You are a character consistency grader for Jeff, an AI companion.
Check the following Jeff response for violations of Jeff's character spec.

Check for exactly these violation types:
- FillerPhrase: contains "Certainly", "Absolutely", "Great question",
  "Of course", "Sure thing", "Happy to help", "I'd be happy to",
  "I'll go ahead and", "I've gone ahead and"
- PermissionSeeking: seeks permission to state an opinion ("Would it be
  okay if I", "If you'd like I could", "I might suggest", "Perhaps I could")
- DisagreementAsQuestion: frames a disagreement as a question ("Have you
  considered", "You might want to think about whether", "Wouldn't it be
  better to")
- TrailingSummary: ends by summarizing what Jeff just did ("So I've gone
  ahead and", "I've now revised the paragraph to", "In summary, I have")
- ResultWithoutAssessment: delivers a revision, draft, or task result without
  a first-person assessment sentence before the result (not applicable to
  conversational replies)
- ExcessiveHedge: uses more than one hedge clause for a single opinion
- NonAnswer: says "it depends" or equivalent without providing an actual
  answer
- SelfNarration: narrates its own process before delivering ("I'll now
  analyze", "First, let me examine", "Let me take a look at")

Respond only with JSON. No other text.
{"violations": ["ViolationType", ...], "explanation": "one sentence"}
If no violations: {"violations": [], "explanation": "clean"}"#;

#[derive(Debug, Deserialize)]
struct CharacterEvalCase {
    id: String,
    #[allow(dead_code)]
    context: String,
    input: String,
    jeff_output: String,
    violations: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GraderVerdict {
    violations: Vec<String>,
    #[allow(dead_code)]
    explanation: String,
}

struct Args {
    cases_path: PathBuf,
    pass_bar: usize,
    system_prompt_path: Option<PathBuf>,
}

fn main() {
    dotenvy::dotenv().ok();
    env::set_var(jeff_desktop::secrets::PREFER_ENV_OPENAI_KEY_VAR, "1");

    match run() {
        Ok(true) => process::exit(0),
        Ok(false) => process::exit(1),
        Err(err) => {
            eprintln!("character eval error: {err:#}");
            process::exit(2);
        }
    }
}

fn run() -> Result<bool> {
    let args = parse_args()?;
    if jeff_desktop::secrets::openai_api_key_from_env().is_none() {
        return Err(anyhow!("OPENAI_API_KEY is required for character eval"));
    }

    let cases = read_cases(&args.cases_path)?;
    if cases.is_empty() {
        return Err(anyhow!("character eval sample is empty"));
    }

    let system_prompt = match args.system_prompt_path {
        Some(path) => fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?,
        None => DEFAULT_GRADER_SYSTEM_PROMPT.to_string(),
    };

    let router = ModelRouter::new(openai_judgment_router_config());
    let mut passed = 0usize;
    let negative_count = cases
        .iter()
        .filter(|case| !case.violations.is_empty())
        .count();

    for case in &cases {
        let verdict = grade_case(&router, &system_prompt, case)
            .with_context(|| format!("failed to grade {}", case.id))?;
        let case_passed = case_passes(case, &verdict);
        if case_passed {
            passed += 1;
            println!("[PASS] {}", case.id);
        } else if case.violations.is_empty() {
            println!(
                "[FAIL] {} - expected clean, got {:?}",
                case.id, verdict.violations
            );
        } else {
            println!(
                "[FAIL] {} - expected violations, got {:?}",
                case.id, verdict.violations
            );
        }
    }

    println!(
        "{}/{} passed; {} negative cases sampled",
        passed,
        cases.len(),
        negative_count
    );
    Ok(passed >= args.pass_bar)
}

fn parse_args() -> Result<Args> {
    let mut raw = env::args().skip(1);
    let cases_path = raw.next().map(PathBuf::from).ok_or_else(|| {
        anyhow!("usage: character_eval <cases.json> [--pass-bar N] [--system-prompt PATH]")
    })?;
    let mut pass_bar = 13usize;
    let mut system_prompt_path = None;

    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--pass-bar" => {
                let value = raw
                    .next()
                    .ok_or_else(|| anyhow!("--pass-bar requires a value"))?;
                pass_bar = value
                    .parse::<usize>()
                    .context("failed to parse --pass-bar")?;
            }
            "--system-prompt" => {
                let value = raw
                    .next()
                    .ok_or_else(|| anyhow!("--system-prompt requires a path"))?;
                system_prompt_path = Some(PathBuf::from(value));
            }
            other => return Err(anyhow!("unknown argument: {other}")),
        }
    }

    Ok(Args {
        cases_path,
        pass_bar,
        system_prompt_path,
    })
}

fn read_cases(path: &PathBuf) -> Result<Vec<CharacterEvalCase>> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let cases: Vec<CharacterEvalCase> =
        serde_json::from_str(&raw).context("failed to parse character eval cases")?;
    Ok(cases)
}

fn openai_judgment_router_config() -> RouterConfig {
    let model =
        env::var("JEFF_CHARACTER_EVAL_MODEL").unwrap_or_else(|_| OPENAI_FALLBACK_MODEL.to_string());
    let openai = TierConfig {
        provider: ProviderKind::OpenAi,
        model,
    };
    RouterConfig {
        reflex: TierConfig {
            provider: ProviderKind::Local,
            model: jeff_desktop::model_router::DEFAULT_REFLEX_MODEL.to_string(),
        },
        conversation: openai.clone(),
        judgment: openai.clone(),
        craft: openai,
    }
}

fn grade_case(
    router: &ModelRouter,
    system_prompt: &str,
    case: &CharacterEvalCase,
) -> Result<GraderVerdict> {
    let user_prompt = format!(
        "User input:\n{}\n\nJeff response to grade:\n{}",
        case.input.trim(),
        case.jeff_output.trim()
    );
    let mut request = ModelRequest::new(Tier::Judgment, system_prompt, user_prompt).with_options(
        GenerateOptions {
            temperature: 0.0,
            max_tokens: Some(220),
            json_object: true,
            timeout_ms: Some(30_000),
        },
    );
    request.purpose = Some("character_eval".to_string());
    let response = router.route(request)?;
    parse_verdict(&response.text)
}

fn parse_verdict(raw: &str) -> Result<GraderVerdict> {
    match serde_json::from_str(raw) {
        Ok(verdict) => Ok(verdict),
        Err(_) => {
            let json = extract_json_object(raw)?;
            serde_json::from_str(json).context("failed to parse grader JSON verdict")
        }
    }
}

fn extract_json_object(raw: &str) -> Result<&str> {
    let start = raw
        .find('{')
        .ok_or_else(|| anyhow!("grader output did not contain '{{'"))?;
    let end = raw
        .rfind('}')
        .ok_or_else(|| anyhow!("grader output did not contain '}}'"))?;
    if end <= start {
        return Err(anyhow!("grader output had invalid JSON object bounds"));
    }
    Ok(&raw[start..=end])
}

fn case_passes(case: &CharacterEvalCase, verdict: &GraderVerdict) -> bool {
    if case.violations.is_empty() {
        verdict.violations.is_empty()
    } else {
        !verdict.violations.is_empty()
    }
}
