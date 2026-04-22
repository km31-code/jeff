pub fn slugify_title(title: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;

    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_dash = false;
            continue;
        }

        if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }

    let trimmed = slug.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "task".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::slugify_title;

    #[test]
    fn slugifies_titles_into_workspace_safe_values() {
        assert_eq!(slugify_title("History StoryMap"), "history-storymap");
        assert_eq!(slugify_title("  APUSH: Unit #7  "), "apush-unit-7");
        assert_eq!(slugify_title("***"), "task");
    }
}
