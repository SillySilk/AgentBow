import { useStore } from "../store";

export default function ProgressLog() {
  const scrape = useStore((s) => s.scrape);
  if (!scrape.running && !scrape.finished) return null;
  return (
    <div style={{ marginTop: 16 }}>
      <div style={{ color: "#a8b2d8", marginBottom: 6 }}>
        {scrape.phase} — {scrape.done}/{scrape.target} downloaded
        {scrape.error ? ` · ${scrape.error}` : ""}
      </div>
      <pre style={{ background: "#16213e", color: "#8893b8", padding: 10, borderRadius: 8, maxHeight: 200, overflow: "auto", fontSize: 12 }}>
        {scrape.log.join("\n")}
      </pre>
    </div>
  );
}
