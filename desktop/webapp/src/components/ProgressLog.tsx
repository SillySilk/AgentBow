import { Radio } from "lucide-react";
import { useStore } from "../store";

const KNOWN_SOURCES = ["yandex", "brave", "ddg", "bing"];

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
    </div>
  );
}
