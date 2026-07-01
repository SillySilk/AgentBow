# Handoff: Agent 008 "Bojangles Hornsnailer" — Image Forge (Cockpit)

## Overview
This is a full visual rebrand of **Bow Image Studio** (the `AgentBow` local image
scraper/curation web UI) into **Thing‑o‑Matic · Agent 008 "Bojangles
Hornsnailer"** — an underground‑speakeasy‑meets‑forge‑workshop theme with a slight
James Bond nod. Same features, same data flow, same backend contract; only the
chrome, copy, and styling change.

The redesign is a single screen — **The Hunt** (the cockpit) — which is the whole
current app: search/scrape controls, page‑scrape, live progress log, and the
curation grid, arranged in a branded sidebar shell.

**Nothing about the backend, WebSocket protocol, REST endpoints, or store logic
needs to change.** This is a presentation‑layer swap. Reuse the existing
`store.ts`, `api.ts`, and all handlers as‑is; only the JSX and styles are new.

## About the Design Files
The files in this bundle are **design references created in HTML/CSS** — a
prototype showing the intended look and behavior. They are **not** production code
to copy directly.

Your task is to **recreate this design inside the existing `desktop/webapp`
React + TypeScript app**, using its established patterns (Zustand store,
functional components, inline‑style or your preferred styling approach). The HTML
reference is built on the Thing‑o‑Matic design system; you should reproduce its
*values* (colors, type, spacing, radii) via the included `design-tokens.css`, not
by pulling in the whole design system.

- `Agent 008 — Cockpit.dc.html` — the reference screen. Open in a browser to see
  the target. (It loads the design‑system bundle from a `_ds/…` path that only
  exists in the design tool — the visuals are what matters, not that it runs
  standalone.)
- `design-tokens.css` — **the important one.** All colors, fonts, gradients,
  radii, and glows the design uses, self‑contained. Import this (or port the
  values into your theme) and you have the whole palette.
- `assets/` — the three real image assets used (see **Assets** below).

## Fidelity
**High‑fidelity (hifi).** Final colors, typography, spacing, and copy. Recreate
pixel‑close using the tokens provided. Where the reference uses the design
system's React components (Button, Switch, Tag), you should build equivalent
small components in the webapp styled to match — you do **not** need to depend on
the design‑system package.

---

## The rebrand in one glance (term mapping)

The functional labels were renamed into character but kept intuitive. When you
rewrite the JSX copy, use the right column. Underlying variables/handlers keep
their current names.

- **Bow Image Studio** → **Agent 008 · Image Forge** (wordmark)
- Search query → **The subject** (`query`)
- Count → **Haul size** (`count`)
- Destination folder → **The vault** / drop point (`destDir`)
- Sources (Brave/DuckDuckGo/Yandex/Bing) → **Informants** (`sources`)
- Add to a specific bin → **Vault** targeting (`bin`)
- Skip duplicates already in the bin → **No doubles** (`dedupe`)
- Vision‑QA (local model checks each image) → **The Inspector** (`verify` + `visionPrompt`)
- Delay between downloads → **Cadence** (`delayMs`)
- **Download images** → **STOKE THE FORGE — RUN THE HAUL** (primary action)
- Scrape a page / gallery → **Field Job** (page‑scrape panel)
- Open browser → **Ghost car** (`openBrowser`)
- Scrape images from current page → **Work the gallery** (`pageScrape`)
- Progress log → **The Wire**
- Curation grid → **The Lineup**
- Working slot → **Vault** tabs (the numbered slots)
- Delete selected → **Burn selected**
- Remove duplicates → **Cull doubles**
- Open folder → **Open vault**
- Backend status "connected" → **line secure** / **machine warm**

Voice: terse, dry, conspiratorial. Second person. ALL‑CAPS + wide letter‑spacing
only for typewriter eyebrows/labels/buttons. No emoji, ever. See the design
system voice notes ("Ask for it by name.", "Pull the lever.", "Tell no one.").

---

## Screens / Views

### Screen: The Hunt (cockpit) — replaces the entire current `App.tsx` body

**Purpose:** The operator types a subject, picks informants and options, and
stokes the forge to scrape + inspect + dedupe images into a vault, watching the
wire and curating the lineup — identical workflow to today.

**Layout:** Full‑viewport CSS grid, two columns.
- Root: `display:grid; grid-template-columns:232px 1fr; height:100vh; width:100vw;
  overflow:hidden; background:var(--surface-forge-base);
  color:var(--text-forge-body); font-family:var(--font-body);`
- **Left (232px): sidebar** — vertical flex, `background:var(--surface-forge-side);
  border-right:1px solid var(--border-forge)`.
- **Right (1fr): main** — vertical flex, `position:relative`, dark base with a
  faint ember texture: `background-image:linear-gradient(180deg,
  rgba(10,10,11,.72), rgba(10,10,11,.88)), url(assets/bg_embers.png);
  background-size:cover`. Contains header → pipeline rail → scrollable body.
- **Body:** CSS grid `grid-template-columns:1.5fr 1fr; grid-template-rows:auto 1fr;
  gap:18px; padding:18px 26px 22px`.
  - The Mark panel spans `grid-column:1; grid-row:1 / span 2` (tall left column).
  - Field Job card at `grid-column:2; grid-row:1`.
  - The Wire + The Lineup stack at `grid-column:2; grid-row:2`.

#### Component: Sidebar
- Padding `20px 18px 12px` for the brand block.
- **Brand block:** flex row, gap 11px. `emblem.png` at `height:44px`. Text block
  `line-height:1.1`: wordmark "Agent 008" in `var(--font-display)`, 21px,
  `color:var(--gold-500)`, `white-space:nowrap`; subtitle "IMAGE FORGE" in
  `var(--font-type)`, 9.5px, `letter-spacing:.16em`, `color:var(--text-forge-mute)`,
  `margin-top:5px`.
- **Gold hairline** under brand: `height:1px; background:var(--rule-gold);
  opacity:.5; margin:4px 14px 8px`.
- **Nav group headers** ("THE JOB", "THE HOUSE"): `var(--font-type)`, 9.5px,
  `letter-spacing:.2em`, `color:var(--text-forge-faint)`, padding `8px 20px 4px`.
- **Nav items:** flex row, gap 11px, `height:44px`, padding `0 20px`,
  `var(--font-type)` 12.5px `letter-spacing:.06em`. Each has a Lucide icon at
  18×18. 
  - **Active** ("The Hunt"): `background:var(--surface-forge-lit);
    color:var(--gold-400); border-left:3px solid var(--gold-500)`.
  - **Inactive:** `color:var(--text-forge-mute); border-left:3px solid transparent`.
  - Items + icons: The Hunt `crosshair`, Field Job `binoculars`, The Wire `radio`,
    The Lineup `layout-grid`, The Vault `archive`, Console `message-square-more`.
    (Only "The Hunt" is wired in this screen; the rest are nav scaffolding —
    keep or trim to match how you route.)
- **Footer (agent chip):** `margin-top:auto`, `border-top:1px solid
  var(--border-forge)`, padding 14px, flex gap 11px. A 36px circle avatar:
  `background:var(--midnight-700); border:1.5px solid var(--gold-700);
  color:var(--gold-400); font-family:var(--font-display); font-size:17px`, text
  "08". Beside it: "Bojangles" (`var(--font-body)`, 13px,
  `color:var(--text-forge-cream)`) and a status line "LINE SECURE"
  (`var(--font-type)`, 9px, `color:var(--absinthe)`) preceded by a 6px absinthe
  dot with `box-shadow:0 0 7px var(--absinthe)`. **Drive this text from the
  store's `status`**: connected → "LINE SECURE", else "LINE DOWN" (ember).

#### Component: Header bar
- `background:var(--surface-forge-head); border-bottom:1px solid
  var(--border-forge); padding:18px 26px 14px`; flex, `align-items:flex-end;
  justify-content:space-between`.
- Left: eyebrow "ASSIGNMENT · LIVE" (`var(--font-type)`, 10px,
  `letter-spacing:.2em`, `color:var(--ember-400)`); title "The Hunt"
  (`var(--font-display)`, 34px, `color:var(--gold-500)`, `line-height:1`).
- Right: a status pill "machine warm" — `var(--font-type)` 10px uppercase
  `color:var(--text-forge-mute)`, preceded by a 7px `var(--flame-500)` dot with
  glow. Then a **ghost Button** "New haul" (resets the form).

#### Component: Pipeline rail (Mark → Haul → Inspect → Cull)
Four equal flex cells, gap 8px, padding `12px 26px`, `border-bottom:1px solid
var(--border-forge)`. Each cell: padding `9px 12px`, `border-radius:var(--radius-forge-md)`,
a 20px numbered/checked circle + a `var(--font-type)` 11px label.
- **Done** (THE MARK): `background:var(--surface-forge-lit); border:1px solid
  var(--gold-700)`; circle `background:var(--gold-500); color:#1a1206` with a "✓";
  label `color:var(--gold-400)`.
- **Active** (THE HAUL): `background:var(--surface-forge-well); border:1px solid
  var(--gold-700)`; circle `background:var(--surface-forge-card);
  color:var(--flame-500); border:1px solid var(--flame-500)`, "2"; label
  `color:var(--text-forge-cream)`.
- **Pending** (INSPECT, CULL): `background:var(--surface-forge-well); border:1px
  solid var(--border-forge)`; circle bordered `var(--border-forge)`,
  `color:var(--forge-idle)`; label `color:var(--text-forge-mute)`.
Wire the "active" step to the live scrape phase if you want; otherwise it's a
static status indicator.

#### Component: The Mark panel — **this is the old `SearchPanel.tsx`**
Brushed‑metal card: `background:linear-gradient(180deg, rgba(20,19,18,.6),
rgba(20,19,18,.92)), url(assets/panel_metal.png); background-size:cover;
border:1px solid var(--border-forge); border-radius:var(--radius-forge-lg);
padding:20px`; flex column.
- **Header row:** label "THE MARK" (`var(--font-type)`, 12px,
  `letter-spacing:.2em`, `color:var(--gold-500)`) + a marker scrawl "what are we
  after?" (`var(--font-marker)`, 15px, `color:var(--ember-400)`,
  `transform:rotate(-3deg)`).
- **Field pattern (reuse for every input):** a `var(--font-type)` 10px uppercase
  label (`letter-spacing:.12em; color:var(--text-forge-mute)`) above the control.
  Inputs: `background:var(--surface-forge-well); color:var(--text-forge-cream);
  border:1px solid var(--border-forge-cool); border-radius:var(--radius-forge-sm);
  padding:10px 11px; font-size:14px; font-family:var(--font-body)`.
  - **The subject** → `query` (full‑width text input).
  - Row `grid-template-columns:96px 1fr`: **Haul size** → `count` (number),
    **The vault (drop point)** → `destDir` (text).
- **Informants** → the `sources` checkboxes, rendered as **Tag chips**. Selected
  = filled marker chip (gold), unselected = plain outline chip. In the reference:
  Yandex + Brave selected (marker style), DuckDuckGo + Bing unselected (plain
  style). Clicking a chip toggles membership in the `enabled` set. (See Tag spec
  below.)
- **Options group** (bordered top+bottom, `var(--border-forge)`, gap 12px):
  - **The Inspector** (title `var(--font-body)` 14px cream; sublabel "LOCAL EYES
    CHECK EVERY FRAME" `var(--font-type)` 9.5px mute) + a **Switch** →
    `verify`. When on, also surface the `visionPrompt` textarea (styled like the
    other wells) — keep today's behavior.
  - **No doubles** (sublabel "DITCH VISUAL DUPES IN THE VAULT") + **Switch** →
    `dedupe`.
  - **Cadence** slider: a header row with "CADENCE · BETWEEN GRABS" and the value
    ("1.5s") in `var(--gold-400)`, then a 6px track (`background:var(--surface-forge-well);
    border:1px solid var(--border-forge); border-radius:99px`) with a fill using
    `var(--grad-progress)`. → `delayMs` (0–10000, step 250; display `/1000`).
  - (Keep the existing "Add to a specific bin" control if you want it — map it to
    a **Vault** selector. It's omitted from the reference for density; re‑add in
    the options group styled like the other rows.)
- **Primary action** (`margin-top:auto`): a **forge Button**, block, size lg,
  label "▾ STOKE THE FORGE — RUN THE HAUL" → today's `startScrape(...)` with the
  same payload. Disabled logic unchanged (running || not connected || no query ||
  no destDir || no sources). Wrap it in an element carrying the `forgeGlow`
  pulse animation (see Interactions).

#### Component: Field Job card — **the old `PageScrapePanel.tsx`**
`background:var(--surface-forge-card); border:1px solid var(--border-forge);
border-radius:var(--radius-forge-lg); padding:16px`.
- Header: "FIELD JOB" (gold typewriter label) + "WORK A GALLERY" (faint mute).
- Row: a URL text input (well style, flex:1) + an **ember Button** "Ghost car" →
  `openBrowser(url)`.
- A status line "TAIL OPEN · PROFILE PERSISTS" (`var(--font-type)` 9.5px
  `color:var(--absinthe)`) with a `lock` Lucide icon — show when `browserUrl` is
  set (mirror today's "Browser open at: …").
- A block **ghost Button** "Work the gallery · 30 rounds" → `pageScrape({count,
  destDir, scrolls})`. (Keep the count/scrolls/destDir inputs from today — either
  inline in this card or behind the button; the reference compresses them.)

#### Component: The Wire — **the old `ProgressLog.tsx`**
`background:var(--surface-forge-well); border:1px solid var(--border-forge);
border-radius:var(--radius-forge-lg); padding:12px 14px`.
- Header: `radio` icon + "THE WIRE" (gold typewriter label, `letter-spacing:.16em`).
- Log body: `var(--font-type)`, 11px, `line-height:1.85; color:var(--text-forge-mute)`.
  Render each `scrape_event`/progress line here. Color cues: source names in
  `var(--absinthe)`, "inspector" rejections in `var(--gold-400)`, the running
  summary line in `var(--text-forge-cream)` with "running…" in `var(--flame-400)`.
  Auto‑scroll to bottom (same as today).

#### Component: The Lineup — **the old `CurationGrid.tsx`**
`background:var(--surface-forge-card); border:1px solid var(--border-forge);
border-radius:var(--radius-forge-lg); padding:12px 14px`; flex column, fills
remaining height.
- Header: "THE LINEUP" label + **Vault tabs** on the right (the working slots).
  Active vault = pill `background:var(--gold-500); color:#1a1206`; inactive =
  `background:var(--surface-forge-well); border:1px solid var(--border-forge);
  color:var(--text-forge-mute)`. Each pill shows name + count, e.g. "VAULT 1 · 44".
  → `slots` list, `setWorkingSlot(path)`, active = `workingSlotDir`.
- **Thumbnail grid:** `display:grid; grid-template-columns:repeat(5,1fr);
  grid-auto-rows:minmax(52px,1fr); gap:6px; overflow:auto`. Each tile:
  `border-radius:4px`, image `object-fit:cover` (use `thumbUrl(it.path)`).
  - **Selected** tile: `border:2px solid var(--gold-500)` (today's red selection
    → now gold).
  - **Flagged/reject** tile: `border:1px solid var(--ember-600)` with a small
    "redaction bar" strip near the bottom (`background:var(--ink-900);
    transform:rotate(-4deg)`) — optional flavor; map to whatever "rejected/dupe"
    marker you keep.
- **Toolbar** (below grid, gap 6px): **danger Button** "Burn (N)" →
  `deleteImages([...selected])` (disabled when none selected); **ghost Button**
  "Cull doubles" → `dedupe(dir, true)`; **ghost Button** "Open vault" →
  `openFolder(dir)`. (Keep a Refresh affordance too — an IconButton with a
  `rotate-cw` icon, or a ghost button, → `refresh()`.)

---

## Interactions & Behavior
All existing behavior is preserved — reuse the handlers in `store.ts`/`api.ts`.
Only two things are net‑new, both cosmetic:

- **Forge‑glow pulse** on the primary "Stoke the Forge" button (and any other
  molten CTA). A 2.4s infinite box‑shadow pulse:
  ```css
  @keyframes forgeGlow {
    0%,100% { box-shadow: 0 0 0 1px var(--gold-600), 0 0 18px rgba(255,122,24,.35),
              inset 0 1px 0 rgba(255,240,192,.5); }
    50%     { box-shadow: 0 0 0 1px #fff0c0, 0 0 34px rgba(255,122,24,.7),
              inset 0 1px 0 rgba(255,240,192,.6); }
  }
  .forge-cta { animation: forgeGlow 2.4s ease-out infinite; }
  @media (prefers-reduced-motion: reduce) { .forge-cta { animation: none; } }
  ```
  Wrap the button (not the button element itself, so it composes with the
  button's own hover shadow).
- **Lucide icons:** render `<i data-lucide="name">` and call
  `lucide.createIcons()` after mount (or use `lucide-react` and import the icons
  directly, which is cleaner in React — `Crosshair`, `Binoculars`, `Radio`,
  `LayoutGrid`, `Archive`, `MessageSquareMore`, `Lock`).

**Component interaction states** (match the design system):
- Buttons — **hover:** brighten `filter:brightness(1.08)` + gold glow on
  primary/forge; **press:** `transform:translateY(1px) scale(0.99)` + dim;
  **focus:** 2px gold ring; **disabled:** `opacity:0.4`, grayscale, no shadow.
- Inputs/selects — **focus:** border → `var(--gold-600)` + inset gold glow.
- Switch — off = dark track; **on = ember track with a gold thumb** + ember glow.
- Tag chips — marker chips sit at a slight rotation (`rotate(-1.5deg)`, alternating).

Motion durations: fast 120ms (hover/press), base 200ms, slow 360ms. Standard
easing `ease-out`; a tiny overshoot only on toggles/checks.

---

## State Management
**No changes.** The existing Zustand store (`desktop/webapp/src/store.ts`) already
holds everything the redesign needs:
- `status` (connection) → drives "line secure / machine warm" and disabled states.
- `scrape.running` / `scrape.finished` → button label ("Scraping…" → "Stoke the
  Forge"), pipeline‑rail active step, grid refresh.
- `startScrape(payload)`, `pageScrape(...)`, `openBrowser(url)`, `browserUrl`.
- Curation: `listImages`, `listSlots`, `thumbUrl`, `deleteImages`, `dedupe`,
  `openFolder`, `setWorkingSlot`, `workingSlotDir`, `lastDestDir`.
- Local component state for the form (`query`, `count`, `destDir`, `enabled`
  set, `verify`, `visionPrompt`, `delayMs`, `bin`, `dedupe`, `scrolls`) — keep
  as‑is; only the labels around them change.

The redesign is purely the `App.tsx` + the four component files' JSX/CSS.

---

## Design Tokens
See **`design-tokens.css`** (bundled) for the complete, self‑contained set. The
values the cockpit actually uses:

**Fonts:** Pirata One (display/blackletter), Permanent Marker (marker scrawl),
Special Elite (typewriter/labels/buttons), Crimson Pro (body/serif). Google Fonts
CDN; the `@import` is in the token file.

**Core colors:** gold‑500 `#d4af37` (primary), gold‑400 `#f4d160`, gold‑700
`#8a5a12`; ember‑500 `#a8311e` (danger/accent), ember‑400 `#c14529`, ember‑600
`#842316`; flame‑500 `#ff7a18` (forge heat / "running"); absinthe `#8fa86b`
(success/line‑secure); parchment‑200 `#e9e0cc` (light text); midnight‑700
`#122146` (avatar).

**Forge surfaces:** base `#0a0a0b`, side `#08080a`, head `#0d0c0a`, card
`#141312`, well `#100f0d`, lit `#161208`. **Forge text:** body `#c6c6ce`, cream
`#e8e0c8`, mute `#8a8a93`, faint `#4a4a44`. **Forge borders:** `#2a2a1e`
(hairline), `#3a3a3f` (inputs). **Frame border:** `#4a3a12` (local).

**Radii:** sm 5 / md 6 (buttons) / lg 8 (cards) / xl 10 (app frame).

**Gradients:** ingot `linear-gradient(180deg,#f6c453,#b8860b)` (forge button),
molten‑hot `…(#ffd874,#a8701a)` (hover), progress
`linear-gradient(90deg,#8a5a12,#d4972b,#f6c453)`, rule‑gold (gold hairline).

**Spacing:** 4px base scale; the layout uses 6/8/12/14/16/18/20/26px paddings and
gaps as noted per component.

**Button variants used:** `forge` (molten ingot gradient, dark text, gold border,
the primary CTA), `ember` (burnt‑red, "Ghost car"), `ghost` (transparent, gold
text/border — "New haul", "Work the gallery", "Cull doubles", "Open vault"),
`danger` (ember outline — "Burn"). Sizes: sm ~30px, md ~40px, lg ~52px.

---

## Assets
Three real image assets (from the Thing‑o‑Matic forge layer), bundled under
`assets/`. Drop them into `desktop/webapp/public/` (or your asset pipeline) and
reference by URL.
- `emblem.png` — the flame‑over‑anvil emblem (also `emblem.svg`). Used in the
  sidebar brand block (44px) and the avatar concept. **This is the Agent 008 mark.**
- `bg_embers.png` — a dark rising‑embers texture. Used as the main content
  backdrop under a dark linear‑gradient overlay (`background-size:cover`).
- `panel_metal.png` — a brushed‑metal texture. Used behind The Mark panel under a
  dark overlay.

Icons are **Lucide** (already an easy `lucide-react` dependency). No emoji.

If you have real product imagery for thumbnails, the grid already uses
`thumbUrl(path)` from `api.ts` — nothing to add.

---

## Files
In this bundle:
- `Agent 008 — Cockpit.dc.html` — the hifi visual reference (open in a browser).
- `design-tokens.css` — all tokens, self‑contained (**start here**).
- `assets/emblem.png`, `assets/emblem.svg`, `assets/bg_embers.png`,
  `assets/panel_metal.png` — image assets.

In the target repo (`SillySilk/AgentBow`) you will touch:
- `desktop/webapp/src/App.tsx` — replace the outer shell + `<h1>` with the
  sidebar grid + header + pipeline rail; mount the panels into the body grid.
- `desktop/webapp/src/components/SearchPanel.tsx` → **The Mark** panel.
- `desktop/webapp/src/components/PageScrapePanel.tsx` → **Field Job** card.
- `desktop/webapp/src/components/ProgressLog.tsx` → **The Wire**.
- `desktop/webapp/src/components/CurationGrid.tsx` → **The Lineup** (vault tabs,
  gold selection, renamed toolbar).
- `desktop/webapp/src/index.css` — add the font `@import`, the token `:root`
  block, and the `forgeGlow` keyframes; set `body { background:#0a0a0b; margin:0 }`.
- Add `lucide-react` (or the Lucide CDN) for icons.

**Do not** modify anything under `desktop/src-tauri/` (the Rust backend), the
WebSocket/REST contract, or the store's data logic. This is a skin.
