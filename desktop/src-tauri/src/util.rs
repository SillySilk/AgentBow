//! Small shared helpers.

/// Return the largest prefix of `s` that is at most `max_chars` *characters*
/// (not bytes) long, always cut on a UTF-8 char boundary.
///
/// Rust panics if you slice a `str` at a byte index that lands inside a
/// multi-byte character (`&s[..n]`). This is the safe replacement: it counts
/// characters and slices at the resulting boundary, so it never panics on
/// emoji, accented text, CJK, smart quotes, etc.
pub fn char_prefix(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s, // fewer than max_chars characters — return all of it
    }
}

/// Truncate `s` to at most `max_chars` characters, appending a note with the
/// original character count when truncation occurred. Never panics.
pub fn truncate_with_note(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    if total <= max_chars {
        return s.to_string();
    }
    format!(
        "{}\n\n[... truncated — {} total chars]",
        char_prefix(s, max_chars),
        total
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_prefix() {
        assert_eq!(char_prefix("hello world", 5), "hello");
        assert_eq!(char_prefix("hi", 5), "hi");
        assert_eq!(char_prefix("", 5), "");
    }

    #[test]
    fn does_not_panic_on_multibyte_boundary() {
        // "é" is 2 bytes; "😀" is 4 bytes. A byte slice at &s[..1] would panic.
        let s = "é😀café— déjà vu 日本語テスト";
        for n in 0..=s.chars().count() + 3 {
            let p = char_prefix(s, n); // must not panic
            assert!(p.chars().count() <= n);
            assert!(s.starts_with(p));
        }
    }

    #[test]
    fn truncate_note_only_when_needed() {
        assert_eq!(truncate_with_note("short", 10), "short");
        let long = "a".repeat(20);
        let out = truncate_with_note(&long, 10);
        assert!(out.starts_with(&"a".repeat(10)));
        assert!(out.contains("20 total chars"));
    }
}
