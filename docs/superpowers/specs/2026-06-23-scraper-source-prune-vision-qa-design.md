# Scraper source pruning + vision-QA inline gate — design

**Date:** 2026-06-23
**Status:** Approved, implementing
**Branch:** bow-image-studio

## Problem

The image scraper queries six sources, but the results show heavy redundancy and
unreliable connectors:

- **DDG** (100) and **Bing** (35) draw from the *same* Bing image index — near-duplicate pictures.
- **Qwant** returns `403 Forbidden` (bot-blocked; also partly Bing-derived).
- **SearXNG** returns `500` (depends on a flaky public instance; it's a meta-aggregator, so redundant by design).
- **Yandex** returns `0` (captcha/anti-bot wall; the parser finds nothing and reports no error).
- Only **Brave** (58) pulls from a genuinely independent index.

Two further issues:

1. Downloads run **3-wide concurrently with no delay** (`download_urls_to_dir`), which is
   impolite to image hosts and unnecessary given the user is not time-constrained.
2. No quality/relevance curation — every downloadable URL is kept regardless of whether it
   matches the query or is usable.

## Goals

1. Cut redundant/broken sources; focus scraping effort on **independent indexes**.
2. Revive Yandex (an independent index distinct from Bing/Brave).
3. Add an optional **vision-QA inline gate** using the existing LM Studio connection so each
   image is checked for relevance + quality before being kept.
4. Slow pacing down (configurable) to be polite and reduce bot-detection risk.

Non-goals: adding new image *APIs* (Wikimedia/Pexels/etc.), Google Images scraping, or
self-hosting SearXNG. Comprehensive open-web coverage via independent search indexes only.

## A. Source set changes

| Source | Action | Rationale |
|---|---|---|
| Brave | Keep, enabled | Only independent index currently working |
| DDG | Keep, enabled | Serves Bing index at 100 results — volume workhorse |
| Yandex | Keep, enabled, **revive** | Independent index, distinct pictures |
| Bing | **Keep code, UI default OFF** | Redundant with DDG; retained as one-click fallback if DDG breaks |
| Qwant | **Remove** | 403 bot-blocked + Bing-derived (redundant) |
| SearXNG | **Remove** | Flaky public instance + meta-aggregator (redundant) |

Edits:
- `image_search.rs`: delete `scrape_qwant_images`, `scrape_searxng_images`, their
  `source_enabled` dispatch lines, and update the canonical-key doc comment.
- `SearchPanel.tsx`: remove `qwant` + `searxng` from `ALL_SOURCES`; initialize the `enabled`
  set to exclude `bing` (default off) while leaving it in the list as a toggle.

## B. Yandex revival

Runs on the user's **residential IP**, where Yandex anti-bot is far less aggressive than from
a datacenter — a plain-HTTP revival has a real chance.

- Send a complete modern browser header set (`sec-ch-ua`, `sec-ch-ua-mobile`,
  `sec-ch-ua-platform`, `sec-fetch-*`, `upgrade-insecure-requests`, updated Chrome UA).
- During implementation, fetch a live Yandex response and confirm the current
  `serp-item` / `data-bem` JSON shape; update the `img_href` (and fallback) extraction to match.
- **Captcha detection:** if the response is the SmartCaptcha interstitial (detect marker such
  as `SmartCaptcha`, `showcaptcha`, `/checkcaptcha`), return an explicit
  `ScrapeResult::err("Yandex", "captcha challenge — skipped")` instead of silent 0.
- Escalation (not in this pass): route Yandex through the existing `ControlledBrowser` if
  plain-HTTP proves unreliable.

## C. Vision-QA inline gate

**Config (`state.rs`):** add `lm_studio_vision_model: String`, read from env
`LM_STUDIO_VISION_MODEL`, falling back to `lm_studio_model` when unset.

**Flow** — when `verify` is enabled, downloads become **sequential** (concurrency 1):

```
for each candidate (until `count` approved OR pool exhausted):
    bytes = download_image_bytes(url)        # existing
    verdict = vision_judge(bytes, prompt)    # KEEP | DISCARD + reason
    if verdict.keep:  save to dest; approved += 1; emit Downloaded
    else:             drop bytes; emit Failed{reason: "vision: <reason>"}
    sleep(delay_ms)
```

- Reuses `call_vision_model` (already transcodes WebP→PNG, caps at 4 MB).
- Candidate pool stays `count * 4`. If the pool is exhausted before `count` approvals, finish
  with what was approved and report `approved/requested` in the log + `Done` event.
- **Default judging prompt** (editable in UI) instructs the model to evaluate:
  relevance to the query, technical quality (sharp, adequate resolution, not an upscaled
  thumbnail), and absence of junk (watermark, logo, collage/grid, heavy text overlay, UI
  chrome). The query string is interpolated into the prompt.
- **Verdict parsing:** ask for strict JSON `{"keep": bool, "reason": "..."}`. Parse leniently
  (find first `{...}`); on parse failure, **keep the image with a flagged reason** so parser
  bugs never silently discard valid images. Log the raw model reply at debug level.

**Events:** reuse `ScrapeEvent::Downloaded` / `ScrapeEvent::Failed`. Add a
`ScrapeEvent::Verifying { url, done, target }` (optional) so the UI can show "checking image
N". The verdict reason rides on the existing `Failed { reason }`.

## D. Pacing

- New scrape_request field `delay_ms: u64` (default e.g. 1500), surfaced as a **slider** in
  `SearchPanel.tsx`. Applied as a sleep between downloads in the sequential path.
- Small randomized jitter (e.g. 300–800 ms) between search-engine scrapes in `image_download`
  — this is the real bot-detection surface.
- **Fast path preserved:** when `verify == false` AND `delay_ms == 0`, keep the existing
  3-wide concurrent download path. Otherwise use the sequential path.

## E. Wiring summary

- `state.rs` — `lm_studio_vision_model` config + env parse.
- `image_search.rs` — remove 2 scrapers; revive Yandex; extend `image_download` +
  `download_urls_to_dir` signatures with `verify: bool`, `vision_prompt: Option<String>`,
  `delay_ms: u64`, `lm_studio_url`, `vision_model`; implement sequential verify-gate; jitter
  between scrapes; optional `Verifying` event.
- `server.rs` — parse `verify`, `vision_prompt`, `delay_ms` from `scrape_request`; pass config
  `lm_studio_url` + `lm_studio_vision_model` into `image_download`.
- `store.ts` — extend `startScrape` payload with `verify`, `visionPrompt`, `delayMs`.
- `SearchPanel.tsx` — update `ALL_SOURCES` (drop qwant/searxng, Bing default-off); add verify
  toggle (default on), editable judging-prompt textarea, delay slider.

## Testing

- Rust: `cargo build` clean (no warnings — project rule). Unit-test the verdict JSON parser
  (keep/discard/malformed→keep-flagged) and the Yandex captcha-detection branch with fixture
  HTML. Network scrapers are validated manually against live responses.
- Manual: run a search with verify on (vision model loaded) and confirm rejects are dropped
  with reasons; run with verify off + delay 0 to confirm the fast path still works; confirm
  Bing toggle is off by default and Qwant/SearXNG are gone from the UI.
