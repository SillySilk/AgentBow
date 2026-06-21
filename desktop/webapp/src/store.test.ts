import { describe, it, expect } from "vitest";
import { applyEvent, initialScrapeState, isBrowserOpened } from "./store";

describe("applyEvent", () => {
  it("accumulates downloaded files and tracks done count", () => {
    let s = initialScrapeState();
    s = applyEvent(s, { type: "scrape_event", kind: "phase", label: "Downloading" });
    s = applyEvent(s, { type: "scrape_event", kind: "downloaded", done: 1, target: 3, path: "C:\\x\\a.jpg" });
    s = applyEvent(s, { type: "scrape_event", kind: "downloaded", done: 2, target: 3, path: "C:\\x\\b.jpg" });
    expect(s.downloaded).toEqual(["C:\\x\\a.jpg", "C:\\x\\b.jpg"]);
    expect(s.done).toBe(2);
    expect(s.target).toBe(3);
  });

  it("marks finished on done", () => {
    let s = initialScrapeState();
    s = applyEvent(s, { type: "scrape_event", kind: "done", downloaded: ["a"], log_note: "Log: x" });
    expect(s.finished).toBe(true);
    expect(s.running).toBe(false);
  });
});

describe("isBrowserOpened", () => {
  it("detects browser_opened", () => {
    expect(isBrowserOpened({ type: "browser_opened", url: "https://x" })).toBe(true);
    expect(isBrowserOpened({ type: "scrape_event", kind: "done", downloaded: [], log_note: "" })).toBe(false);
  });
});
