//! Deterministic conversation titles.
//!
//! Titles are derived once from the first user message; no model call is made.
//! The stored value is at most 120 characters; the visible title is at most 80
//! characters, trimmed at a word boundary.

/// Maximum stored title length.
pub const MAX_TITLE_CHARS: usize = 120;

/// Maximum derived title length.
pub const DERIVED_TITLE_CHARS: usize = 80;

/// Derives a title from the first user message.
///
/// Whitespace is collapsed, the text is truncated to
/// [`DERIVED_TITLE_CHARS`] at a word boundary when possible, and an ellipsis is
/// appended when anything was removed.
pub fn derive_title(content: &str) -> String {
    let collapsed = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return String::new();
    }
    let chars: Vec<char> = collapsed.chars().collect();
    if chars.len() <= DERIVED_TITLE_CHARS {
        return collapsed;
    }
    let mut end = DERIVED_TITLE_CHARS;
    while end > 1 && !chars[end - 1].is_whitespace() {
        end -= 1;
    }
    if end <= 1 {
        end = DERIVED_TITLE_CHARS;
    }
    let mut title: String = chars[..end].iter().collect();
    while title.ends_with(' ') {
        title.pop();
    }
    format!("{title}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_short_titles() {
        assert_eq!(derive_title("  Hello   world "), "Hello world");
    }

    #[test]
    fn truncates_at_a_word_boundary() {
        let long = "word ".repeat(40);
        let title = derive_title(&long);
        assert!(title.ends_with('…'));
        assert!(title.chars().count() <= DERIVED_TITLE_CHARS + 1);
        assert!(!title.contains("wor…"));
    }

    #[test]
    fn handles_empty() {
        assert_eq!(derive_title("   "), "");
    }

    #[test]
    fn keeps_a_title_at_the_limit() {
        let title = "a".repeat(DERIVED_TITLE_CHARS);
        assert_eq!(derive_title(&title), title);
    }

    #[test]
    fn truncates_a_single_long_word_at_the_limit() {
        let title = derive_title(&"a".repeat(DERIVED_TITLE_CHARS + 10));
        assert_eq!(title.chars().count(), DERIVED_TITLE_CHARS + 1);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn truncates_at_a_space_before_the_limit() {
        let mut content = "a".repeat(DERIVED_TITLE_CHARS - 5);
        content.push(' ');
        content.push_str("tail words that are removed");
        let title = derive_title(&content);
        assert!(title.ends_with('…'));
        assert_eq!(title.chars().count(), DERIVED_TITLE_CHARS - 5 + 1);
    }

    #[test]
    fn counts_unicode_characters_not_bytes() {
        let content = "é ".repeat(60);
        let title = derive_title(&content);
        assert!(title.chars().count() <= DERIVED_TITLE_CHARS + 1);
    }
}
