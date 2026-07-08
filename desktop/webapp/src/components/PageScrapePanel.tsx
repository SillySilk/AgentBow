import { useState } from "react";
import { Lock } from "lucide-react";
import { useStore } from "../store";
import Button from "./ui/Button";
import CasePanel from "./CasePanel";

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
    <div style={{ background: "var(--surface-forge-card)", border: "1px solid var(--border-forge)", borderRadius: "var(--radius-forge-lg)", padding: 16 }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: 12 }}>
        <span style={{ fontFamily: "var(--font-type)", color: "var(--gold-500)", fontSize: 12, letterSpacing: ".2em" }}>FIELD JOB</span>
        <span style={{ fontFamily: "var(--font-type)", fontSize: 9.5, letterSpacing: ".06em", color: "var(--text-forge-mute)" }}>WORK A GALLERY</span>
      </div>

      <div style={{ display: "flex", gap: 8, marginBottom: 10 }}>
        <input placeholder="gallery.example.net/db5 (log in / navigate first)" value={url} onChange={(e) => setUrl(e.target.value)}
          className="forge-input" style={{ flex: 1, fontSize: 13, minWidth: 0 }} />
        <Button variant="ember" size="sm" disabled={!ready || !url.trim()} onClick={() => openBrowser(url)}>Ghost car</Button>
      </div>

      {browserUrl && (
        <div style={{ display: "flex", alignItems: "center", gap: 8, fontFamily: "var(--font-type)", fontSize: 9.5, letterSpacing: ".06em", color: "var(--absinthe)", marginBottom: 12 }}>
          <Lock size={13} />
          TAIL OPEN · {browserUrl}
        </div>
      )}

      <div style={{ display: "flex", gap: 8, marginBottom: 10 }}>
        <input type="number" min={1} max={500} value={count} onChange={(e) => setCount(Math.max(1, Math.min(500, Number(e.target.value) || 1)))}
          className="forge-input" style={{ width: 70, fontSize: 13 }} title="max images" />
        <input type="number" min={0} max={50} value={scrolls} onChange={(e) => setScrolls(Math.max(0, Math.min(50, Number(e.target.value) || 0)))}
          className="forge-input" style={{ width: 70, fontSize: 13 }} title="scroll passes" />
        <input placeholder="Destination folder" value={destDir} onChange={(e) => setDestDir(e.target.value)}
          className="forge-input" style={{ flex: 1, fontSize: 13, minWidth: 0 }} />
      </div>

      <Button variant="ghost" size="sm" block disabled={!ready || running || !destDir.trim()} onClick={() => pageScrape({ count, destDir, scrolls })}>
        {running ? "Working…" : `Work the gallery · ${count} rounds`}
      </Button>

      <CasePanel url={url} destDir={destDir} count={count} scrolls={scrolls} />
    </div>
  );
}
