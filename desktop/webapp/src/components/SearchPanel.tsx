import { useState } from "react";
import { useStore } from "../store";
import Button from "./ui/Button";
import Switch from "./ui/Switch";
import Tag from "./ui/Tag";

const ALL_SOURCES = [
  { key: "yandex", label: "Yandex" },
  { key: "brave", label: "Brave" },
  { key: "ddg", label: "DuckDuckGo" },
  { key: "bing", label: "Bing" },
];
// Default to Yandex only: its safe-search-off cookie is confirmed working (uncensored
// results), and since downloads fill from the first engine's candidates, a single
// uncensored source guarantees uncensored downloads. Toggle the others on as needed.
const DEFAULT_ENABLED = ["yandex"];

const DEFAULT_PROMPT_HINT =
  "Leave blank to use the default: judges relevance to the subject, technical quality, and rejects watermarks/collages/text overlays.";

export default function SearchPanel() {
  const startScrape = useStore((s) => s.startScrape);
  const status = useStore((s) => s.status);
  const running = useStore((s) => s.scrape.running);
  const engine = useStore((s) => s.engine);
  const visionDisabled = !!engine && !engine.vision;
  const [query, setQuery] = useState("");
  const [count, setCount] = useState(15);
  const [destDir, setDestDir] = useState("C:\\AI\\workspace\\");
  const [enabled, setEnabled] = useState<Set<string>>(new Set(DEFAULT_ENABLED));
  const [verify, setVerify] = useState(true);
  const [visionPrompt, setVisionPrompt] = useState("");
  const [delayMs, setDelayMs] = useState(1500);
  const [useBin, setUseBin] = useState(false);
  const [bin, setBin] = useState(1);
  const [dedupe, setDedupe] = useState(true);

  const disabled = running || status !== "connected" || !query.trim() || !destDir.trim() || enabled.size === 0;
  const toggleSource = (key: string) => setEnabled((prev) => {
    const n = new Set(prev);
    if (n.has(key)) n.delete(key); else n.add(key);
    return n;
  });

  return (
    <div
      style={{
        height: "100%",
        backgroundColor: "var(--surface-forge-card)",
        backgroundImage: "linear-gradient(180deg, rgba(20,19,18,.6), rgba(20,19,18,.92)), url('./panel_metal.png')",
        backgroundSize: "cover",
        border: "1px solid var(--border-forge)",
        borderRadius: "var(--radius-forge-lg)",
        padding: 20,
        display: "flex",
        flexDirection: "column",
      }}
    >
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: 16 }}>
        <span style={{ fontFamily: "var(--font-type)", color: "var(--gold-500)", fontSize: 12, letterSpacing: ".2em" }}>THE MARK</span>
        <span style={{ fontFamily: "var(--font-marker)", color: "var(--ember-400)", fontSize: 15, transform: "rotate(-3deg)" }}>what are we after?</span>
      </div>

      <label className="forge-label">The subject</label>
      <input
        className="forge-input"
        placeholder="e.g. 1965 aston martin db5, silver, studio"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        style={{ width: "100%", margin: "5px 0 14px" }}
      />

      <div style={{ display: "grid", gridTemplateColumns: "96px 1fr", gap: 12, marginBottom: 14 }}>
        <div>
          <label className="forge-label">Haul size</label>
          <input type="number" min={1} max={200} className="forge-input" style={{ width: "100%", marginTop: 5 }}
            value={count} onChange={(e) => setCount(Math.max(1, Math.min(200, Number(e.target.value) || 1)))} />
        </div>
        <div>
          <label className="forge-label">The vault (drop point)</label>
          <input className="forge-input" style={{ width: "100%", marginTop: 5 }}
            value={destDir} onChange={(e) => setDestDir(e.target.value)} placeholder="Destination folder" />
        </div>
      </div>

      <label className="forge-label" style={{ marginBottom: 8 }}>Informants</label>
      <div style={{ display: "flex", gap: 8, flexWrap: "wrap", marginBottom: 16 }}>
        {ALL_SOURCES.map((s) => (
          <Tag key={s.key} active={enabled.has(s.key)} onClick={() => toggleSource(s.key)}>{s.label}</Tag>
        ))}
      </div>

      <div style={{ display: "flex", flexDirection: "column", gap: 12, padding: "14px 0", borderTop: "1px solid var(--border-forge)", borderBottom: "1px solid var(--border-forge)", marginBottom: 16 }}>
        <OptionRow title="The Inspector" sublabel={visionDisabled ? "LOADED MODEL HAS NO VISION" : "LOCAL EYES CHECK EVERY FRAME"}>
          <span title={visionDisabled ? "Loaded model has no vision" : undefined}>
            <Switch checked={verify} onChange={setVerify} disabled={visionDisabled} label="The Inspector" />
          </span>
        </OptionRow>
        {verify && (
          <textarea placeholder={DEFAULT_PROMPT_HINT} value={visionPrompt} onChange={(e) => setVisionPrompt(e.target.value)}
            rows={3} className="forge-input" style={{ width: "100%", resize: "vertical", fontFamily: "var(--font-body)" }} />
        )}

        <OptionRow title="No doubles" sublabel="DITCH VISUAL DUPES IN THE VAULT">
          <Switch checked={dedupe} onChange={setDedupe} label="No doubles" />
        </OptionRow>

        <OptionRow title="Target a vault" sublabel="APPEND TO A SPECIFIC NUMBERED VAULT">
          <Switch checked={useBin} onChange={setUseBin} label="Target a vault" />
        </OptionRow>
        {useBin && (
          <select className="forge-input" value={bin} onChange={(e) => setBin(Number(e.target.value))} style={{ width: 120 }}>
            {Array.from({ length: 10 }, (_, i) => i + 1).map((n) => (
              <option key={n} value={n}>Vault {n}</option>
            ))}
          </select>
        )}

        <div>
          <div style={{ display: "flex", justifyContent: "space-between", fontFamily: "var(--font-type)", fontSize: 10, letterSpacing: ".08em", color: "var(--text-forge-mute)", marginBottom: 6 }}>
            <span>CADENCE · BETWEEN GRABS</span>
            <span style={{ color: "var(--gold-400)" }}>{(delayMs / 1000).toFixed(1)}s{delayMs === 0 ? " (fastest)" : ""}</span>
          </div>
          <input type="range" min={0} max={10000} step={250} value={delayMs} onChange={(e) => setDelayMs(Number(e.target.value))} className="forge-range" />
        </div>
      </div>

      <div className="forge-cta" style={{ marginTop: "auto" }}>
        <Button
          variant="forge" size="lg" block
          disabled={disabled}
          onClick={() => startScrape({ query, count, destDir, sources: [...enabled], delayMs, verify, visionPrompt, bin: useBin ? bin : null, dedupe })}
        >
          {running ? "▾ STOKING…" : "▾ STOKE THE FORGE — RUN THE HAUL"}
        </Button>
      </div>
    </div>
  );
}

function OptionRow({ title, sublabel, children }: { title: string; sublabel: string; children: React.ReactNode }) {
  return (
    <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
      <div style={{ lineHeight: 1.3 }}>
        <div style={{ fontFamily: "var(--font-body)", color: "var(--text-forge-cream)", fontSize: 14 }}>{title}</div>
        <div style={{ fontFamily: "var(--font-type)", fontSize: 9.5, letterSpacing: ".06em", color: "var(--text-forge-mute)" }}>{sublabel}</div>
      </div>
      {children}
    </div>
  );
}
