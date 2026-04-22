pub fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.is_empty() || right.is_empty() || left.len() != right.len() {
        return 0.0;
    }

    let mut dot = 0.0f32;
    let mut left_norm = 0.0f32;
    let mut right_norm = 0.0f32;

    for (l, r) in left.iter().zip(right.iter()) {
        dot += l * r;
        left_norm += l * l;
        right_norm += r * r;
    }

    let denom = left_norm.sqrt() * right_norm.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        dot / denom
    }
}

#[cfg(test)]
mod tests {
    use super::cosine_similarity;

    #[test]
    fn cosine_similarity_scores_expected_ordering() {
        let query = vec![1.0, 1.0, 0.0];
        let close = vec![0.9, 1.1, 0.0];
        let far = vec![0.0, 0.0, 1.0];

        let close_score = cosine_similarity(&query, &close);
        let far_score = cosine_similarity(&query, &far);

        assert!(close_score > far_score);
    }

    #[test]
    fn cosine_similarity_handles_invalid_vectors() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0);
    }
}
