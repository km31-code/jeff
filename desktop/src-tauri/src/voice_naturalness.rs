// phase 22: deterministic spoken-output cleanup. this module only touches text
// that is sent to tts; written chat output remains exactly what the llm returned.

pub const DEFAULT_TTS_VOICE: &str = "alloy";

const AVAILABLE_TTS_VOICES: &[&str] = &[
    "alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer",
    "verse",
];

const FILLER_PHRASES: &[&str] = &[
    "great question",
    "good question",
    "sure thing",
    "of course",
    "certainly",
    "absolutely",
    "i'd be happy to",
    "i would be happy to",
    "i'm happy to",
    "happy to help",
    "happy to assist",
    "i'd be glad to",
    "no problem",
    "let me help you with that",
    "great to hear",
    "sounds good",
    "of course, i",
    "certainly, i",
];

const INTERJECTIONS: &[&str] = &[
    "got it",
    "on it",
    "here you go",
    "sure",
    "understood",
    "right",
    "makes sense",
    "yep",
];

pub fn available_tts_voices() -> Vec<String> {
    AVAILABLE_TTS_VOICES
        .iter()
        .map(|voice| (*voice).to_string())
        .collect()
}

pub fn normalize_tts_voice(value: &str) -> String {
    let normalized = value.trim().to_ascii_lowercase();
    if AVAILABLE_TTS_VOICES
        .iter()
        .any(|voice| *voice == normalized)
    {
        normalized
    } else {
        DEFAULT_TTS_VOICE.to_string()
    }
}

pub fn remove_tts_filler_phrases(text: &str) -> String {
    let mut cleaned = text.to_string();
    for phrase in FILLER_PHRASES {
        cleaned = remove_phrase_case_insensitive(&cleaned, phrase);
    }
    normalize_tts_spacing(&cleaned)
}

pub fn prepare_tts_text(text: &str, seed: &str) -> String {
    let cleaned = remove_tts_filler_phrases(text);
    if cleaned.is_empty() {
        return cleaned;
    }

    if word_count(&cleaned) < 15 && !starts_with_interjection(&cleaned) {
        let interjection = deterministic_interjection(seed, &cleaned);
        format!("{interjection}. {cleaned}")
    } else {
        cleaned
    }
}

pub fn deterministic_interjection(seed: &str, text: &str) -> &'static str {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in seed.bytes().chain(text.bytes()) {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    INTERJECTIONS[(hash as usize) % INTERJECTIONS.len()]
}

pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

fn starts_with_interjection(text: &str) -> bool {
    let lower = text.trim_start().to_ascii_lowercase();
    INTERJECTIONS.iter().any(|prefix| {
        if !lower.starts_with(prefix) {
            return false;
        }
        // the interjection must be followed by end-of-string or a non-alphanumeric
        // character (any punctuation, space, em dash, etc.) so that words that
        // merely start with an interjection word do not trigger suppression.
        let rest = &lower[prefix.len()..];
        rest.is_empty() || rest.starts_with(|c: char| !c.is_alphanumeric())
    })
}

fn remove_phrase_case_insensitive(text: &str, phrase: &str) -> String {
    let mut result = String::new();
    let lower = text.to_ascii_lowercase();
    let phrase_lower = phrase.to_ascii_lowercase();
    let mut cursor = 0;

    while let Some(relative) = lower[cursor..].find(&phrase_lower) {
        let start = cursor + relative;
        let phrase_end = start + phrase_lower.len();
        if !is_boundary(text, start) || !is_boundary_or_punctuation(text, phrase_end) {
            result.push_str(&text[cursor..phrase_end]);
            cursor = phrase_end;
            continue;
        }

        result.push_str(&text[cursor..start]);
        let mut end = phrase_end;
        while end < text.len() {
            let Some(ch) = text[end..].chars().next() else {
                break;
            };
            if matches!(ch, ',' | '.' | '!' | '?' | ':' | ';') {
                end += ch.len_utf8();
            } else {
                break;
            }
        }
        cursor = end;
    }

    result.push_str(&text[cursor..]);
    result
}

fn is_boundary(text: &str, index: usize) -> bool {
    if index == 0 || index >= text.len() {
        return true;
    }
    let Some(ch) = text[..index].chars().next_back() else {
        return true;
    };
    !ch.is_alphanumeric()
}

fn is_boundary_or_punctuation(text: &str, index: usize) -> bool {
    if index >= text.len() {
        return true;
    }
    let Some(ch) = text[index..].chars().next() else {
        return true;
    };
    !ch.is_alphanumeric() || matches!(ch, ',' | '.' | '!' | '?' | ':' | ';')
}

fn normalize_tts_spacing(text: &str) -> String {
    let mut compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for (from, to) in [
        (" ,", ","),
        (" .", "."),
        (" !", "!"),
        (" ?", "?"),
        (" ;", ";"),
        (" :", ":"),
    ] {
        compact = compact.replace(from, to);
    }
    compact
        .trim()
        .trim_start_matches(|ch: char| matches!(ch, ',' | '.' | '!' | '?' | ':' | ';' | ' '))
        .trim_end_matches(|ch: char| matches!(ch, ',' | ':' | ';' | ' '))
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filler_removal_is_case_insensitive_and_punctuation_safe() {
        let cleaned = remove_tts_filler_phrases(
            "Of course! Great question, absolutely: the rubric wants evidence.",
        );
        assert_eq!(cleaned, "the rubric wants evidence.");
    }

    #[test]
    fn filler_removal_does_not_strip_inside_words() {
        let cleaned = remove_tts_filler_phrases("This is uncertainly worded.");
        assert_eq!(cleaned, "This is uncertainly worded.");
    }

    #[test]
    fn short_tts_text_gets_stable_interjection() {
        let first = prepare_tts_text("Use the primary source.", "turn-1");
        let second = prepare_tts_text("Use the primary source.", "turn-1");
        assert_eq!(first, second);
        assert!(first.contains(". Use the primary source"));
    }

    #[test]
    fn long_tts_text_does_not_get_interjection() {
        let prepared = prepare_tts_text(
            "This answer has enough words that it should stay direct without an added spoken acknowledgment.",
            "turn-2",
        );
        for interjection in super::INTERJECTIONS {
            assert!(
                !prepared.starts_with(&format!("{interjection}.")),
                "interjection '{interjection}' should not be prepended to long text"
            );
        }
    }

    #[test]
    fn starts_with_interjection_handles_varied_punctuation() {
        // dot-space is the obvious case but the guard should cover all non-alpha chars.
        assert!(starts_with_interjection("Got it! here is what you need"));
        assert!(starts_with_interjection("on it, looking now"));
        assert!(starts_with_interjection("understood — let me check"));
        assert!(starts_with_interjection("sure"));
        // partial word must not match.
        assert!(!starts_with_interjection("surely this is not an interjection"));
    }

    #[test]
    fn filler_removal_catches_expanded_phrases() {
        let cleaned = remove_tts_filler_phrases("Happy to help! Here is the answer.");
        assert_eq!(cleaned, "Here is the answer.");
        let cleaned2 = remove_tts_filler_phrases("No problem, let me explain.");
        assert_eq!(cleaned2, "let me explain.");
        // should not strip mid-sentence fragments
        let cleaned3 = remove_tts_filler_phrases("This is certainly true.");
        assert_eq!(cleaned3, "This is true.");
    }

    #[test]
    fn invalid_voice_normalizes_to_default() {
        assert_eq!(normalize_tts_voice("nova"), "nova");
        assert_eq!(normalize_tts_voice("not-a-voice"), DEFAULT_TTS_VOICE);
    }
}
