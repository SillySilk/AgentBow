# Site Connections + Edge Default + Bing Re-add — Design

**Date:** 2026-06-24
**Branch:** bow-image-studio
**Status:** Approved design, pending implementation plan

## Problem

Scrapes against Yandex fail with "not authorized" because the headed controlled
browser uses a dedicated, isolated profile (`C:\AI\workspace\.bow_browser_profile`)
that starts logged out. The user has no in-app way to log into that profile, and no
visibility into whether a session exists. Separately, the user now uses Edge as their
main browser and wants Bow to drive Edge by default, and wants Bing back in the
default scrape rotation.

## Goals

1. Make **Edge** the default browser Bow launches (Chrome remains a fallback).
2. Put **Bing** back into the default scrape source set (alongside Yandex).
3. Add an in-app **Connections** UI with **live status** and one-click **login** for
   the sites that have real account logins: **Yandex** and **Bing**.
4. Show **"coming soon" placeholders** for future loginable image sources
   (DeviantArt, Rule34, a generic archive site) as a roadmap signal.

## Non-Goals

- Building new scrapers/parsers for DeviantArt, Rule34, or archive sites (separate
  future spec — "scope B"). The placeholders are inert.
- Login buttons for Brave Search or DuckDuckGo — neither has an account login or a
  session benefit, so neither gets a button.
- Decrypting cookie values. Connection status only needs cookie existence + expiry,
  both of which are plaintext in the cookie DB.

## Existing machinery this reuses

- **Persistent profile already exists** at `.bow_browser_profile` (`state.rs:132-135`).
  Anything logged in there persists across runs.
- **`browser_open` WS command already exists** (`server.rs:265`) — opens the headed
  controlled browser to any URL and returns `{"type":"browser_opened","url":...}`.
- **Bing is already a working scraper** (`image_search.rs:608-611`, `parse_bing`) —
  it is only excluded from the Yandex-only default introduced in commit `058c2f2`.
- **Edge is already a supported executable** in the candidate list
  (`controlled_browser.rs:10-15`); it is simply listed after Chrome.

## Design

### 1. Default browser → Edge

In `controlled_browser.rs` `chrome_executable()`, reorder `CANDIDATES` so the two
`msedge.exe` paths come **first** and the two `chrome.exe` paths come **second**:

```
C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe
C:\Program Files\Microsoft\Edge\Application\msedge.exe
C:\Program Files\Google\Chrome\Application\chrome.exe
C:\Program Files (x86)\Google\Chrome\Application\chrome.exe
```

The `CHROME_PATH` env override (checked before the candidate list) is unchanged, so
the user can still force a specific executable. No other launch logic changes —
`chromiumoxide` drives Edge over CDP identically to Chrome. The error message
("No Chrome/Edge found…") already names both.

### 2. Bing back in the default source set

The default rotation must run **Yandex + Bing**. Today, with no `sources` specified,
`source_enabled` returns true for all engines, but commit `058c2f2` made the effective
default Yandex-only. Restore Bing to the default set so an unspecified scrape runs
Yandex first then Bing. Brave and DDG remain available (selectable) but off by default.

Yandex stays ordered first in `browser_engines` (`image_search.rs:603-616`) so its
confirmed safe-search-off candidates lead the download queue; Bing follows.

### 3. Connection status — direct cookie-store read

New Rust module `desktop/src-tauri/src/connections.rs`.

**Mechanism:** the headed profile stores cookies in a SQLite file at
`<profile>\Default\Network\Cookies`. Cookie *values* are DPAPI-encrypted, but the
`host_key`, `name`, and `expires_utc` columns are plaintext. Connection status only
needs to know whether a live (non-expired) auth cookie exists for a site — no
decryption required.

**Reader:** open the Cookies DB **read-only and immutable** (SQLite
`?immutable=1` / `mode=ro`) so it works even while Edge has the file open, and never
locks or mutates it. For each site, query for the site's auth cookie:

| Site   | auth cookie | host_key match            |
|--------|-------------|---------------------------|
| Yandex | `Session_id`| `.yandex.com` / `.yandex.ru` |
| Bing   | `_U`        | `.bing.com`               |

A matching row whose `expires_utc` is in the future (Chrome epoch:
microseconds since 1601-01-01) ⇒ **Connected**. Missing, expired, or DB-absent ⇒
**Not connected**. (Cookie names are calibrated against a real login during
implementation; the table is the contract, the specific names are tunable. The site
list lives in one place so adding sites later is a table edit.)

The reader is **best-effort**: any error (no DB yet, locked, schema surprise) resolves
to "Not connected" rather than failing the request.

**When status is computed:** on app load, after a login window is opened (so a fresh
login reflects quickly), and on an explicit manual refresh from the UI. No background
polling.

### 4. Login buttons (UI)

New webapp component `desktop/webapp/src/components/ConnectionsPanel.tsx`, surfaced in
the existing UI near the search controls.

- **Active rows:** Yandex and Bing. Each shows the site name, a live status badge
  (green "Connected" / grey "Not connected"), and a "Log in" button.
- Clicking "Log in" sends the existing `browser_open` WS message with the site's
  sign-in URL:
  - Yandex → `https://passport.yandex.com/auth`
  - Bing → `https://login.live.com`
  The headed Edge window opens; the user signs in once; the session persists in the
  profile. After the window opens, the UI requests a status refresh.
- **Placeholder rows:** DeviantArt, Rule34, and "Archive site" rendered greyed-out
  with a "coming soon" label, not clickable.

Store (`store.ts`) gains per-site connection status; `api.ts` gains the
status-request call and reuses the existing browser-open call.

### 5. Wiring (WS protocol)

- **New inbound message** `connection_status` (no payload): server runs the
  `connections.rs` reader and replies with
  `{"type":"connection_status","sites":{"yandex":bool,"bing":bool}}`.
- **Login action** reuses the existing `browser_open` inbound message with the
  sign-in URL — no new message needed.
- New module `connections.rs` keeps cookie logic out of `controlled_browser.rs`,
  which stays focused on CDP/driving the page. The profile path is shared from
  `state.rs` (same dir passed to `ControlledBrowser::new`).

## Data flow

```
App load ─┐
Refresh  ─┼─► WS connection_status ─► connections::read_status(profile_dir)
Login open┘                              │ open Cookies DB (ro, immutable)
                                         │ per-site: auth cookie present & unexpired?
                                         ▼
                          {"type":"connection_status","sites":{...}} ─► store ─► badges

"Log in" click ─► WS browser_open{url: sign-in URL} ─► headed Edge opens
                   user signs in ─► cookies written to profile ─► next status read = Connected
```

## Error handling

- Cookie DB missing / unreadable / locked / unexpected schema → site reports
  "Not connected" (never an error to the user).
- `browser_open` failure surfaces via the existing `scrape_event`/error path
  (`server.rs:266`), unchanged.
- Edge not installed and no Chrome and no `CHROME_PATH` → existing launch error
  ("No Chrome/Edge found. Set CHROME_PATH…").

## Testing

- **`connections.rs` unit tests:** build a fixture SQLite cookie DB with plaintext
  `host_key`/`name`/`expires_utc` rows; assert Connected for a present unexpired
  auth cookie, Not connected for missing, expired, and wrong-domain rows, and Not
  connected when the DB file is absent. Verify Chrome-epoch expiry comparison.
- **`controlled_browser.rs`:** keep existing `chrome_executable_honors_env_override`;
  the candidate reorder needs no new test (order is data).
- **Source-default test:** assert the default (unspecified `sources`) enables both
  Yandex and Bing.
- **Webapp:** ConnectionsPanel renders active rows with badges and placeholder rows
  as disabled; "Log in" dispatches `browser_open` with the correct URL; a
  `connection_status` message updates the badges.

## Open calibration items (resolved during implementation, not blockers)

- Confirm the exact auth cookie name for Yandex (`Session_id`) and Bing (`_U`) against
  a live login in the profile; adjust the site table if a more reliable marker exists.
- Confirm Edge's cookie DB path under the user-data-dir matches Chrome's
  (`Default\Network\Cookies`).
