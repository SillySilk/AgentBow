import { useState } from "react";
import { useStore } from "../store";

export default function PageScrapePanel() {
  const openBrowser = useStore((s) => s.openBrowser);
  const pageScrape = useStore((s) => s.pageScrape);
  const status = useStore((s) => s.status);
  const running = useStore((s) => s.scrape.running);
  const browserUrl = useStore((s) => s.browserUrl);
  const [url, setUrl] = useState("");
  const [count, setCount] = useState(30);
  const [scrolls, setScrolls] = useState(5);
  const [destDir, setDestDir] = useState("C:\\AI\\workspace\\");
  const ready = status === "connected";
  return (
    <div style={{ display: "grid", gap: 8, maxWidth: 560, marginTop: 24, borderTop: "1px solid #2a2a4a", paddingTop: 16 }}>
      <strong style={{ color: "#a8b2d8" }}>Scrape a page / gallery</strong>
      {browserUrl && <div style={{ fontSize: "0.8em", color: "#a8b2d8" }}>Browser open at: {browserUrl}</div>}
      <div style={{ display: "flex", gap: 8 }}>
        <input placeholder="Page URL (log in / navigate first)" value={url} onChange={(e) => setUrl(e.target.value)} style={inp} />
        <button disabled={!ready || !url.trim()} onClick={() => openBrowser(url)} style={btn2}>Open browser</button>
      </div>
      <div style={{ display: "flex", gap: 8 }}>
        <input type="number" min={1} max={500} value={count} onChange={(e) => setCount(Math.max(1, Math.min(500, Number(e.target.value) || 1)))} style={{ ...inp, width: 80 }} title="max images" />
        <input type="number" min={0} max={50} value={scrolls} onChange={(e) => setScrolls(Math.max(0, Math.min(50, Number(e.target.value) || 0)))} style={{ ...inp, width: 80 }} title="scroll passes" />
        <input placeholder="Destination folder" value={destDir} onChange={(e) => setDestDir(e.target.value)} style={{ ...inp, flex: 1 }} />
      </div>
      <button disabled={!ready || running || !destDir.trim()} onClick={() => pageScrape({ count, destDir, scrolls })} style={btn}>
        {running ? "Scraping…" : "Scrape images from current page"}
      </button>
    </div>
  );
}

const inp: React.CSSProperties = { background: "#16213e", color: "#a8b2d8", border: "1px solid #2a2a4a", borderRadius: 8, padding: "8px 10px" };
const btn: React.CSSProperties = { background: "#e94560", color: "white", border: "none", borderRadius: 8, padding: "10px 14px", cursor: "pointer" };
const btn2: React.CSSProperties = { background: "#0f3460", color: "#a8b2d8", border: "1px solid #2a2a4a", borderRadius: 8, padding: "8px 12px", cursor: "pointer" };
