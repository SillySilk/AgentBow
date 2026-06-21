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

// ── DOM distillation helpers ──────────────────────────────────────────────────

/// Extract the readable content of an HTML page for LLM context.
///
/// Tries a Readability-style extraction (dom_smoothie) first — this isolates the
/// main article/body text and drops nav/ads/boilerplate. Falls back to a crude
/// tag-strip for non-article pages or fragments where Readability yields little.
pub fn distill_html(html: &str) -> String {
    if let Ok(mut r) = dom_smoothie::Readability::new(html, None, None) {
        if let Ok(article) = r.parse() {
            let text = article.text_content.trim();
            // Readability returns near-nothing on app-like / non-article pages;
            // only trust it when it found a meaningful amount of text.
            if text.chars().count() >= 200 {
                return truncate_text(text, 8000);
            }
        }
    }
    simple_strip_html(html)
}

/// Crude fallback: strip noise tags and all markup, collapse whitespace.
pub fn simple_strip_html(html: &str) -> String {
    let mut s = html.to_string();

    // Remove block-level noise tags entirely (including content)
    for tag in &["script", "style", "head", "noscript", "svg", "iframe"] {
        s = remove_tag_block(&s, tag);
    }

    // Strip remaining HTML tags, keeping just text
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => {
                in_tag = true;
            }
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }

    // Collapse runs of whitespace/newlines
    let mut collapsed = String::with_capacity(out.len());
    let mut last_ws = false;
    for ch in out.chars() {
        if ch.is_whitespace() {
            if !last_ws {
                collapsed.push('\n');
            }
            last_ws = true;
        } else {
            collapsed.push(ch);
            last_ws = false;
        }
    }

    truncate_text(collapsed.trim(), 8000)
}

/// Remove all occurrences of `<tag ...>...</tag>` from `s` (case-insensitive).
pub fn remove_tag_block(s: &str, tag: &str) -> String {
    let open = format!("<{}", tag);
    let close = format!("</{}>", tag);
    let mut result = String::with_capacity(s.len());
    let lower = s.to_lowercase();
    let mut pos = 0;
    while pos < s.len() {
        if let Some(start) = lower[pos..].find(&open).map(|i| i + pos) {
            result.push_str(&s[pos..start]);
            if let Some(end_rel) = lower[start..].find(&close) {
                pos = start + end_rel + close.len();
            } else {
                // Unclosed tag — skip to end
                break;
            }
        } else {
            result.push_str(&s[pos..]);
            break;
        }
    }
    result
}

/// Truncate text to `max_chars` characters (UTF-8 safe), appending a note if
/// truncated. Tries to cut at a word boundary.
pub fn truncate_text(s: &str, max_chars: usize) -> String {
    let total = s.chars().count();
    if total <= max_chars {
        return s.to_string();
    }
    // Char-safe prefix, then back up to the last whitespace for a clean cut.
    let prefix = char_prefix(s, max_chars);
    let cut = prefix.rfind(char::is_whitespace).unwrap_or(prefix.len());
    format!("{}\n\n[... truncated — {} total chars]", &prefix[..cut], total)
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

    #[test]
    fn distill_extracts_article_drops_scripts() {
        let html = r#"<html><head><title>T</title>
            <script>var x = 'SECRET_SCRIPT_TOKEN';</script>
            <style>.a{color:red}</style></head>
            <body><nav>Home About</nav>
            <article><h1>The Headline</h1>
            <p>This is the first substantial paragraph of real article body text that a
            reader actually cares about, long enough to clear the readability threshold
            so the extractor keeps it rather than falling back.</p>
            <p>A second paragraph continues the article with more meaningful prose so the
            content score stays high and the boilerplate around it is discarded cleanly.</p>
            </article><footer>Copyright</footer></body></html>"#;
        let out = distill_html(html);
        assert!(out.contains("first substantial paragraph"), "missing body: {out}");
        assert!(!out.contains("SECRET_SCRIPT_TOKEN"), "script leaked: {out}");
    }

    #[test]
    fn simple_strip_removes_markup() {
        let out = simple_strip_html("<div><script>bad()</script><p>Hello <b>there</b></p></div>");
        assert!(out.contains("Hello"));
        assert!(out.contains("there"));
        assert!(!out.contains("bad()"), "script not stripped: {out}");
        assert!(!out.contains('<'), "tags not stripped: {out}");
    }
}
