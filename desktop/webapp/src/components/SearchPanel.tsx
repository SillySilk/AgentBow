import { useState } from "react";
import { useStore } from "../store";

const ALL_SOURCES = [
  { key: "bing", label: "Bing" },
  { key: "ddg", label: "DuckDuckGo" },
  { key: "yandex", label: "Yandex" },
  { key: "brave", label: "Brave" },
  { key: "qwant", label: "Qwant" },
  { key: "searxng", label: "SearXNG" },
];

export default function SearchPanel() {
  const startScrape = useStore((s) => s.startScrape);
  const status = useStore((s) => s.status);
  const running = useStore((s) => s.scrape.running);
  const [query, setQuery] = useState("");
  const [count, setCount] = useState(15);
  const [destDir, setDestDir] = useState("C:\\AI\\workspace\\");
  const [enabled, setEnabled] = useState<Set<string>>(new Set(ALL_SOURCES.map((s) => s.key)));

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
      <button disabled={disabled} onClick={() => startScrape({ query, count, destDir, sources: [...enabled] })}
        style={{ ...btn, opacity: disabled ? 0.5 : 1 }}>
        {running ? "Scraping…" : "Download images"}
      </button>
    </div>
  );
}
const inp: React.CSSProperties = { background: "#16213e", color: "#a8b2d8", border: "1px solid #2a2a4a", borderRadius: 8, padding: "8px 10px" };
const btn: React.CSSProperties = { background: "#e94560", color: "white", border: "none", borderRadius: 8, padding: "10px 14px", cursor: "pointer" };
