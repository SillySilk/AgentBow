import { Radio } from "lucide-react";
import { useStore } from "../store";

const KNOWN_SOURCES = ["yandex", "brave", "ddg", "bing"];

// Segment colour per source for the download tally bar (keyed by lowercased label).
const SOURCE_COLORS: Record<string, string> = {
  yandex: "var(--flame-400)",
  bing: "var(--gold-400)",
  brave: "var(--ember-400)",
  ddg: "var(--absinthe)",
};
const sourceColor = (name: string) => SOURCE_COLORS[name.toLowerCase()] ?? "var(--text-forge-mute)";

function colorLine(line: string): React.CSSProperties {
  const head = line.split(":")[0].trim().toLowerCase();
  if (line.startsWith("ERROR") || line.toLowerCase().startsWith("failed")) return { color: "var(--ember-400)" };
  if (KNOWN_SOURCES.includes(head)) return { color: "var(--absinthe)" };
  if (head === "inspector" || line.toLowerCase().includes("filtered")) return { color: "var(--gold-400)" };
  return {};
}

export default function ProgressLog() {
  const scrape = useStore((s) => s.scrape);
  if (!scrape.running && !scrape.finished) return null;
  return (
    <div style={{ background: "var(--surface-forge-well)", border: "1px solid var(--border-forge)", borderRadius: "var(--radius-forge-lg)", padding: "12px 14px" }}>
      <div style={{ display: "flex", alignItems: "center", gap: 7, fontFamily: "var(--font-type)", color: "var(--gold-500)", fontSize: 11, letterSpacing: ".16em", marginBottom: 8 }}>
        <Radio size={14} />
        THE WIRE
      </div>
      <div style={{ fontFamily: "var(--font-type)", fontSize: 11, lineHeight: 1.6, color: "var(--text-forge-cream)", marginBottom: 6 }}>
        {scrape.phase} — {scrape.done}/{scrape.target} forged
        {scrape.running && <span style={{ color: "var(--flame-400)" }}> · running…</span>}
        {scrape.error && <span style={{ color: "var(--ember-400)" }}> · {scrape.error}</span>}
      </div>
      <div style={{ fontFamily: "var(--font-type)", fontSize: 11, lineHeight: 1.85, color: "var(--text-forge-mute)", maxHeight: 160, overflow: "auto" }}>
        {scrape.log.map((line, i) => (
          <div key={i} style={colorLine(line)}>{line}</div>
        ))}
      </div>
      <SourceTally bySource={scrape.downloadedBySource} />
    </div>
  );
}

/** Little stacked bar + counts showing how many images landed from each source. */
function SourceTally({ bySource }: { bySource: Record<string, number> }) {
  const entries = Object.entries(bySource).filter(([, n]) => n > 0).sort((a, b) => b[1] - a[1]);
  const total = entries.reduce((sum, [, n]) => sum + n, 0);
  if (total === 0) return null;
  return (
    <div style={{ marginTop: 10, paddingTop: 8, borderTop: "1px solid var(--border-forge)" }}>
      <div style={{ display: "flex", height: 6, borderRadius: 3, overflow: "hidden", background: "var(--surface-forge-well)" }}>
        {entries.map(([src, n]) => (
          <div key={src} title={`${src}: ${n}`} style={{ width: `${(n / total) * 100}%`, background: sourceColor(src) }} />
        ))}
      </div>
      <div style={{ display: "flex", flexWrap: "wrap", gap: "4px 12px", marginTop: 6, fontFamily: "var(--font-type)", fontSize: 10.5, letterSpacing: ".04em", color: "var(--text-forge-mute)" }}>
        {entries.map(([src, n]) => (
          <span key={src} style={{ display: "inline-flex", alignItems: "center", gap: 5 }}>
            <span style={{ width: 8, height: 8, borderRadius: 2, background: sourceColor(src) }} />
            {src} <span style={{ color: "var(--text-forge-cream)" }}>{n}</span>
          </span>
        ))}
        <span style={{ marginLeft: "auto", color: "var(--gold-400)" }}>{total} total</span>
      </div>
    </div>
  );
}
