import { useState } from "react";
import { useStore } from "../store";

const ALL_SOURCES = [
  { key: "brave", label: "Brave" },
  { key: "ddg", label: "DuckDuckGo" },
  { key: "yandex", label: "Yandex" },
  { key: "bing", label: "Bing" },
];
// Default to Yandex only: its safe-search-off cookie is confirmed working (uncensored
// results), and since downloads fill from the first engine's candidates, a single
// uncensored source guarantees uncensored downloads. Toggle the others on as needed.
const DEFAULT_ENABLED = ["yandex"];

const DEFAULT_PROMPT_HINT =
  "Leave blank to use the default: judges relevance to the query, technical quality, and rejects watermarks/collages/text overlays.";

export default function SearchPanel() {
  const startScrape = useStore((s) => s.startScrape);
  const status = useStore((s) => s.status);
  const running = useStore((s) => s.scrape.running);
  const [query, setQuery] = useState("");
  const [count, setCount] = useState(15);
  const [destDir, setDestDir] = useState("C:\\AI\\workspace\\");
  const [enabled, setEnabled] = useState<Set<string>>(new Set(DEFAULT_ENABLED));
  const [verify, setVerify] = useState(true);
  const [visionPrompt, setVisionPrompt] = useState("");
  const [delayMs, setDelayMs] = useState(1500);

  const disabled = running || status !== "connected" || !query.trim() || !destDir.trim() || enabled.size === 0;
  return (
    <div style={{ display: "grid", gap: 8, maxWidth: 560 }}>
      <input placeholder="Search query (e.g. golden retriever puppies)" value={query}
        onChange={(e) => setQuery(e.target.value)} style={inp} />
      <div style={{ display: "flex", gap: 8 }}>
        <input type="number" min={1} max={200} value={count}
          onChange={(e) => setCount(Math.max(1, Math.min(200, Number(e.target.value) || 1)))}
          style={{ ...inp, width: 90 }} />
        <input placeholder="Destination folder" value={destDir}
          onChange={(e) => setDestDir(e.target.value)} style={{ ...inp, flex: 1 }} />
      </div>
      <div style={{ display: "flex", gap: 10, flexWrap: "wrap", fontSize: 12, color: "#a8b2d8" }}>
        {ALL_SOURCES.map((s) => (
          <label key={s.key} style={{ display: "flex", gap: 4, alignItems: "center" }}>
            <input type="checkbox" checked={enabled.has(s.key)}
              onChange={(e) => setEnabled((prev) => { const n = new Set(prev); e.target.checked ? n.add(s.key) : n.delete(s.key); return n; })} />
            {s.label}
          </label>
        ))}
      </div>

      {/* Vision-QA gate */}
      <label style={{ display: "flex", gap: 6, alignItems: "center", fontSize: 13, color: "#a8b2d8" }}>
        <input type="checkbox" checked={verify} onChange={(e) => setVerify(e.target.checked)} />
        Vision-QA: have the local model check each image (slower, higher quality)
      </label>
      {verify && (
        <textarea placeholder={DEFAULT_PROMPT_HINT} value={visionPrompt}
          onChange={(e) => setVisionPrompt(e.target.value)} rows={3}
          style={{ ...inp, resize: "vertical", fontFamily: "inherit" }} />
      )}

      {/* Pacing */}
      <label style={{ display: "grid", gap: 2, fontSize: 12, color: "#a8b2d8" }}>
        <span>Delay between downloads: {(delayMs / 1000).toFixed(1)}s {delayMs === 0 ? "(fastest)" : ""}</span>
        <input type="range" min={0} max={10000} step={250} value={delayMs}
          onChange={(e) => setDelayMs(Number(e.target.value))} />
      </label>

      <button disabled={disabled} onClick={() => startScrape({ query, count, destDir, sources: [...enabled], delayMs, verify, visionPrompt })}
        style={{ ...btn, opacity: disabled ? 0.5 : 1 }}>
        {running ? "Scraping…" : "Download images"}
      </button>
    </div>
  );
}
const inp: React.CSSProperties = { background: "#16213e", color: "#a8b2d8", border: "1px solid #2a2a4a", borderRadius: 8, padding: "8px 10px" };
const btn: React.CSSProperties = { background: "#e94560", color: "white", border: "none", borderRadius: 8, padding: "10px 14px", cursor: "pointer" };
