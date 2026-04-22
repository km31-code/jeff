pub const DEFAULT_CHUNK_SIZE_CHARS: usize = 2800;
pub const DEFAULT_CHUNK_OVERLAP_CHARS: usize = 400;

pub fn chunk_text(input: &str, chunk_size_chars: usize, overlap_chars: usize) -> Vec<String> {
    let normalized = input.trim();
    if normalized.is_empty() {
        return Vec::new();
    }

    let chars: Vec<char> = normalized.chars().collect();
    if chars.len() <= chunk_size_chars {
        return vec![normalized.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0usize;

    while start < chars.len() {
        let end = usize::min(start + chunk_size_chars, chars.len());
        let chunk: String = chars[start..end].iter().collect();
        chunks.push(chunk);

        if end == chars.len() {
            break;
        }

        let next_start = end.saturating_sub(overlap_chars);
        if next_start <= start {
            break;
        }

        start = next_start;
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::{chunk_text, DEFAULT_CHUNK_OVERLAP_CHARS, DEFAULT_CHUNK_SIZE_CHARS};

    #[test]
    fn chunking_is_deterministic_and_non_empty() {
        let input = "a".repeat(DEFAULT_CHUNK_SIZE_CHARS * 2);

        let first = chunk_text(
            &input,
            DEFAULT_CHUNK_SIZE_CHARS,
            DEFAULT_CHUNK_OVERLAP_CHARS,
        );
        let second = chunk_text(
            &input,
            DEFAULT_CHUNK_SIZE_CHARS,
            DEFAULT_CHUNK_OVERLAP_CHARS,
        );

        assert_eq!(first, second);
        assert!(first.iter().all(|chunk| !chunk.is_empty()));
    }

    #[test]
    fn chunking_applies_overlap_between_adjacent_chunks() {
        let input = "0123456789".repeat(700);
        let chunk_size = 500;
        let overlap = 100;

        let chunks = chunk_text(&input, chunk_size, overlap);

        assert!(chunks.len() >= 2);

        let left_tail: String = chunks[0]
            .chars()
            .rev()
            .take(overlap)
            .collect::<Vec<char>>()
            .into_iter()
            .rev()
            .collect();
        let right_head: String = chunks[1].chars().take(overlap).collect();

        assert_eq!(left_tail, right_head);
    }
}
