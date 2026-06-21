import { useState } from "react";
import { useStore } from "../store";

export default function SearchPanel() {
  const startScrape = useStore((s) => s.startScrape);
  const status = useStore((s) => s.status);
  const running = useStore((s) => s.scrape.running);
  const [query, setQuery] = useState("");
  const [count, setCount] = useState(15);
  const [destDir, setDestDir] = useState("C:\\AI\\workspace\\");

  const disabled = running || status !== "connected" || !query.trim() || !destDir.trim();
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
      <button disabled={disabled} onClick={() => startScrape({ query, count, destDir })}
        style={{ ...btn, opacity: disabled ? 0.5 : 1 }}>
        {running ? "Scraping…" : "Download images"}
      </button>
    </div>
  );
}
const inp: React.CSSProperties = { background: "#16213e", color: "#a8b2d8", border: "1px solid #2a2a4a", borderRadius: 8, padding: "8px 10px" };
const btn: React.CSSProperties = { background: "#e94560", color: "white", border: "none", borderRadius: 8, padding: "10px 14px", cursor: "pointer" };
