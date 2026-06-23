# Browser-primary search scraping — design

**Date:** 2026-06-23
**Status:** Approved (autonomous build authorized), implementing
**Branch:** bow-image-studio

## Problem

Plain-HTTP scraping of search engines is getting bot-blocked (Yandex captcha, Qwant
403 already removed, Bing/Brave degrade over time). `reqwest` is flagged before it
ever receives the results page that holds the original image URLs.

## Insight

The ControlledBrowser (`controlled_browser.rs`) drives the user's **real installed
Chrome** over CDP (`chromiumoxide`) with a persistent profile on a residential IP.
It loads the *same* results page `reqwest` is blocked from — including the embedded
JSON that carries original-resolution image URLs. So: fetch the page with the real
browser, then run the existing per-engine extractors over that HTML. Original URLs
*and* no bot wall. Headed window means the user can solve a captcha by hand and the
scrape continues.

## Decisions (from brainstorming)

- **Scope:** Bing, Brave, Yandex fetched via the real browser. **DuckDuckGo stays on
  its existing HTTP+`i.js` API path** — it isn't blocked, works well (100 results),
  and its parser is API-based, not HTML-based, so browserifying it would yield worse
  results for no gain.
- **Window:** headed (visible), reusing one Chrome window for the whole run.
- **Captcha:** pause and let the user solve it, polling until the challenge clears
  (~2 min timeout), then extract.
- Image **downloads** (JPG bytes) stay on `reqwest`. Vision-QA gate, numbered set
  folders, and pacing are unchanged.

## Architecture

### Parser/fetcher split (`image_search.rs`)

Refactor the three blocked scrapers into **pure parsers** (unit-testable against HTML
fixtures), removing their HTTP fetch wrappers:

- `fn parse_bing(html: &str, max: usize) -> Vec<String>` — `murl` entity/JSON/`data-imgurl`.
- `fn parse_brave(html: &str, max: usize) -> Vec<String>` — `imgs.search.brave.com` hrefs/srcs.
- `fn parse_yandex(html: &str, max: usize) -> Vec<String>` — `img_href` (unescape `\/`) + thumb fallbacks.

DDG keeps `scrape_duckduckgo_images` (HTTP) as-is.

### Browser fetch (`controlled_browser.rs`)

- `pub async fn scrape_search_page(&self, url: &str, scrolls: u32) -> Result<String>` —
  `ensure_launched(false)` (headed), navigate, settle, scroll `scrolls` times to
  lazy-load, return **raw** `page.content()` (not distilled — the JSON parsers need it).
- `pub async fn raw_html(&self) -> Result<String>` — current page content without
  navigating, for captcha polling.

### Captcha detection + wait (`image_search.rs`)

- `fn is_captcha_page(html: &str) -> bool` — markers for Yandex SmartCaptcha, Google
  `/sorry`, generic reCAPTCHA/hCaptcha/Cloudflare. (Replaces `is_yandex_captcha`.)
- `async fn wait_for_captcha_clear(browser, timeout) -> Option<String>` — poll
  `raw_html()` every ~3 s until `!is_captcha_page` or timeout; returns cleared HTML.

### Orchestration (`image_download`)

```
if ddg    { results.push(scrape_duckduckgo_images(&client, query, want)) }   // HTTP
for (key, name, url, parse) in [bing, brave, yandex]:
    if enabled(key):
        results.push(scrape_via_browser(browser, name, &url, want, parse, &progress))
```

`scrape_via_browser`: fetch page → if `is_captcha_page`, emit a Phase event
("Solve the captcha for {engine} in the browser window…") and `wait_for_captcha_clear`
→ run `parse` → `ScrapeResult`. On browser error → `ScrapeResult::err`.

`scrape_jitter` is removed — browser navigation paces engines naturally.

### Threading

`image_download` gains `browser: &ControlledBrowser`. Callers:
- `server.rs` `ScrapeRequest` handler — clone `controlled_browser` into the spawn.
- `tools/mod.rs` `dispatch` `image_download` case — pass its `browser`.

## Error handling

- No Chrome/Edge installed → `ensure_launched` errors clearly; that engine's
  `ScrapeResult` carries the error, others still run (DDG via HTTP always works).
- Captcha never solved within timeout → `ScrapeResult::err(source, "captcha — not solved")`.
- First-run consent/cookie dialogs are NOT captchas; the headed window + persistent
  profile let the user accept once and it's remembered. Noted as a possible first-run
  manual step.

## Testing

- Unit-test `parse_bing` / `parse_brave` / `parse_yandex` against small HTML fixtures
  containing the relevant markers, and `is_captcha_page` (positive + negative).
- `cargo check` clean, zero warnings (project rule).
- Live validation (needs real Chrome) deferred to a real run: confirm a visible window
  opens, engines return URLs, and a Yandex captcha can be solved by hand.

## Out of scope

UI redesign (planned separately later). DDG DOM extraction. Headless/auto-captcha.
