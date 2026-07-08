import { describe, it, expect } from "vitest";
import { applyEvent, initialScrapeState, isBrowserOpened, applyCaseEvent, initialCaseState } from "./store";

describe("applyEvent", () => {
  it("accumulates downloaded files and tracks done count", () => {
    let s = initialScrapeState();
    s = applyEvent(s, { type: "scrape_event", kind: "phase", label: "Downloading" });
    s = applyEvent(s, { type: "scrape_event", kind: "downloaded", done: 1, target: 3, path: "C:\\x\\a.jpg", source: "Yandex" });
    s = applyEvent(s, { type: "scrape_event", kind: "downloaded", done: 2, target: 3, path: "C:\\x\\b.jpg", source: "Bing" });
    expect(s.downloaded).toEqual(["C:\\x\\a.jpg", "C:\\x\\b.jpg"]);
    expect(s.done).toBe(2);
    expect(s.target).toBe(3);
  });

  it("tallies downloads per source", () => {
    let s = initialScrapeState();
    s = applyEvent(s, { type: "scrape_event", kind: "downloaded", done: 1, target: 5, path: "a", source: "Yandex" });
    s = applyEvent(s, { type: "scrape_event", kind: "downloaded", done: 2, target: 5, path: "b", source: "Yandex" });
    s = applyEvent(s, { type: "scrape_event", kind: "downloaded", done: 3, target: 5, path: "c", source: "DDG" });
    expect(s.downloadedBySource).toEqual({ Yandex: 2, DDG: 1 });
  });

  it("marks finished on done", () => {
    let s = initialScrapeState();
    s = applyEvent(s, { type: "scrape_event", kind: "done", downloaded: ["a"], log_note: "Log: x", dest_dir: "C:\\x\\1" });
    expect(s.finished).toBe(true);
    expect(s.running).toBe(false);
  });
});

describe("applyCaseEvent", () => {
  it("stores grid candidates", () => {
    let s = initialCaseState();
    s = applyCaseEvent(s, { type: "case_candidates", stage: "grid", items: [
      { id: 0, preview_url: "https://e/a.jpg", href: "https://e/p/1", selector: "div > a > img", w: 100, h: 100 },
    ]});
    expect(s.stage).toBe("grid");
    expect(s.candidates.length).toBe(1);
  });
  it("stores recipe + match counts", () => {
    let s = initialCaseState();
    s = applyCaseEvent(s, { type: "case_recipe", recipe: { domain: "e", grid_selector: "div > a > img", link_selector: "div > a > img", detail_image_selector: "main > img", scrolls: 0 }, matched: 42, total: 60, grid_url: "https://e/g" });
    expect(s.stage).toBe("recipe");
    expect(s.matched).toBe(42);
    expect(s.recipe?.grid_selector).toBe("div > a > img");
    expect(s.gridUrl).toBe("https://e/g");
  });
});

describe("isBrowserOpened", () => {
  it("detects browser_opened", () => {
    expect(isBrowserOpened({ type: "browser_opened", url: "https://x" })).toBe(true);
    expect(isBrowserOpened({ type: "scrape_event", kind: "done", downloaded: [], log_note: "", dest_dir: "" })).toBe(false);
  });
});
