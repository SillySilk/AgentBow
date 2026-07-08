//! Pure, browser-free logic for the "Case the gallery" flow: candidate/recipe
//! types, selector generalization, and the per-domain playbook JSON store.
//!
//! No LLM, no browser, no network — everything here is unit-testable in isolation
//! (see the design spec 2026-07-08-case-the-gallery). The generalization heuristic
//! is deliberately simple: two elements belong to the same "grid" if their CSS
//! paths are equal once every `:nth-of-type(k)` index is stripped.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One extractable element found on a page. `selector` is the CSS path of the
/// *repeating unit* — the wrapping `<a href>` when the image is a link, else the
/// `<img>` itself. `preview_url` is the best thumbnail URL (absolute).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Candidate {
    pub id: usize,
    pub preview_url: String,
    pub href: Option<String>,
    pub selector: String,
    pub w: u32,
    pub h: u32,
}

/// A reusable, URL-independent per-domain extraction recipe.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Recipe {
    pub domain: String,
    pub grid_selector: String,
    /// `Some` ⇒ grid items are links; follow each item's `href` to a detail page.
    pub link_selector: Option<String>,
    /// Structural pattern of the full-size image on a detail page.
    pub detail_image_selector: Option<String>,
    #[serde(default)]
    pub scrolls: u32,
}

/// Strip every `:nth-of-type(k)` index from a CSS path, yielding its structural
/// pattern — the v1 generalization key. Pure string scan (no regex dep).
pub fn structural_pattern(selector: &str) -> String {
    let mut out = String::with_capacity(selector.len());
    let bytes = selector.as_bytes();
    let needle = ":nth-of-type(";
    let mut i = 0;
    while i < selector.len() {
        if selector[i..].starts_with(needle) {
            if let Some(close) = selector[i..].find(')') {
                i += close + 1;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Candidates whose structural pattern equals `pattern`.
pub fn match_pattern<'a>(pattern: &str, all: &'a [Candidate]) -> Vec<&'a Candidate> {
    all.iter()
        .filter(|c| structural_pattern(&c.selector) == pattern)
        .collect()
}

/// Build a recipe + sibling set from one demonstrated example. `detail_image_selector`
/// is left `None` here; the caller fills it after the detail-page demo.
pub fn generalize(
    example: &Candidate,
    all: &[Candidate],
    scrolls: u32,
    domain: &str,
) -> (Recipe, Vec<Candidate>) {
    let grid_selector = structural_pattern(&example.selector);
    let siblings: Vec<Candidate> = match_pattern(&grid_selector, all)
        .into_iter()
        .cloned()
        .collect();
    let link_selector = example.href.as_ref().map(|_| grid_selector.clone());
    let recipe = Recipe {
        domain: domain.to_string(),
        grid_selector,
        link_selector,
        detail_image_selector: None,
        scrolls,
    };
    (recipe, siblings)
}

/// Structural pattern of a detail-page image the user clicked.
pub fn detail_selector_from(c: &Candidate) -> String {
    structural_pattern(&c.selector)
}

/// Registrable-ish domain (host, lowercased, `www.` stripped) or "unknown".
pub fn domain_of(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => u
            .host_str()
            .map(|h| h.trim_start_matches("www.").to_lowercase())
            .unwrap_or_else(|| "unknown".into()),
        Err(_) => "unknown".into(),
    }
}

// ── Playbook store ──────────────────────────────────────────────────────────

fn sanitize_domain(domain: &str) -> String {
    domain
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

pub fn playbook_file(dir: &Path, domain: &str) -> PathBuf {
    dir.join(format!("{}.json", sanitize_domain(domain)))
}

pub fn load_playbooks(dir: &Path, domain: &str) -> Vec<Recipe> {
    let path = playbook_file(dir, domain);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<Recipe>>(&s).ok())
        .unwrap_or_default()
}

/// Append `recipe` to its domain file, replacing any existing recipe with the
/// same `grid_selector` (so re-saving an updated demo overwrites cleanly).
pub fn save_playbook(dir: &Path, recipe: &Recipe) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut all = load_playbooks(dir, &recipe.domain);
    all.retain(|r| r.grid_selector != recipe.grid_selector);
    all.push(recipe.clone());
    let json = serde_json::to_string_pretty(&all).unwrap_or_else(|_| "[]".into());
    std::fs::write(playbook_file(dir, &recipe.domain), json)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: usize, sel: &str, href: Option<&str>, w: u32, h: u32) -> Candidate {
        Candidate {
            id,
            preview_url: format!("https://e.com/{}.jpg", id),
            href: href.map(|s| s.to_string()),
            selector: sel.into(),
            w,
            h,
        }
    }

    #[test]
    fn structural_pattern_strips_indices() {
        assert_eq!(
            structural_pattern("div#g > div:nth-of-type(3) > a > img"),
            "div#g > div > a > img"
        );
        assert_eq!(structural_pattern("ul > li:nth-of-type(12)"), "ul > li");
        assert_eq!(structural_pattern("img"), "img");
    }

    #[test]
    fn generalize_collects_siblings_and_marks_links() {
        let all = vec![
            c(0, "div#g > div:nth-of-type(1) > a > img", Some("https://e.com/p/1"), 100, 100),
            c(1, "div#g > div:nth-of-type(2) > a > img", Some("https://e.com/p/2"), 100, 100),
            c(2, "header > a > img", None, 20, 20), // chrome, different pattern
        ];
        let (recipe, sibs) = generalize(&all[0], &all, 4, "e.com");
        assert_eq!(recipe.grid_selector, "div#g > div > a > img");
        assert_eq!(recipe.link_selector.as_deref(), Some("div#g > div > a > img"));
        assert_eq!(recipe.scrolls, 4);
        assert_eq!(sibs.len(), 2);
    }

    #[test]
    fn generalize_without_href_has_no_link_selector() {
        let all = vec![c(0, "main > img:nth-of-type(1)", None, 800, 600)];
        let (recipe, sibs) = generalize(&all[0], &all, 0, "e.com");
        assert!(recipe.link_selector.is_none());
        assert_eq!(sibs.len(), 1);
    }

    #[test]
    fn domain_of_strips_www() {
        assert_eq!(domain_of("https://www.Example.com/a/b?x=1"), "example.com");
        assert_eq!(domain_of("https://gallery.example.net/db5"), "gallery.example.net");
        assert_eq!(domain_of("not a url"), "unknown");
    }

    #[test]
    fn playbook_round_trips_and_dedupes() {
        let dir = std::env::temp_dir().join(format!("bow_pb_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let r = Recipe {
            domain: "e.com".into(),
            grid_selector: "div > a > img".into(),
            link_selector: Some("div > a > img".into()),
            detail_image_selector: Some("main > img".into()),
            scrolls: 3,
        };
        save_playbook(&dir, &r).unwrap();
        save_playbook(&dir, &r).unwrap(); // same grid_selector ⇒ no duplicate
        let loaded = load_playbooks(&dir, "e.com");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], r);
        assert!(load_playbooks(&dir, "other.com").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
