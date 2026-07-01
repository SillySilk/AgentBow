import { useEffect, useState } from "react";
import { Crosshair, Binoculars, Radio, LayoutGrid, Archive, MessageSquareMore } from "lucide-react";
import { useStore } from "./store";
import SearchPanel from "./components/SearchPanel";
import PageScrapePanel from "./components/PageScrapePanel";
import ProgressLog from "./components/ProgressLog";
import CurationGrid from "./components/CurationGrid";
import Button from "./components/ui/Button";

const NAV_ITEMS = [
  { icon: Crosshair, label: "The Hunt", active: true },
  { icon: Binoculars, label: "Field Job", active: false },
  { icon: Radio, label: "The Wire", active: false },
  { icon: LayoutGrid, label: "The Lineup", active: false },
] as const;
const HOUSE_ITEMS = [
  { icon: Archive, label: "The Vault" },
  { icon: MessageSquareMore, label: "Console" },
] as const;

export default function App() {
  const connect = useStore((s) => s.connect);
  const status = useStore((s) => s.status);
  const running = useStore((s) => s.scrape.running);
  const [resetKey, setResetKey] = useState(0);
  useEffect(() => { connect(); }, [connect]);

  const lineSecure = status === "connected";

  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "232px 1fr",
        height: "100vh",
        width: "100vw",
        overflow: "hidden",
        background: "var(--surface-forge-base)",
        color: "var(--text-forge-body)",
        fontFamily: "var(--font-body)",
      }}
    >
      {/* SIDEBAR */}
      <aside style={{ background: "var(--surface-forge-side)", borderRight: "1px solid var(--border-forge)", display: "flex", flexDirection: "column" }}>
        <div style={{ padding: "20px 18px 12px", display: "flex", alignItems: "center", gap: 11 }}>
          <img src="./emblem.png" alt="Agent 008" style={{ height: 44 }} />
          <div style={{ lineHeight: 1.1 }}>
            <div style={{ fontFamily: "var(--font-display)", color: "var(--gold-500)", fontSize: 21, whiteSpace: "nowrap" }}>Agent 008</div>
            <div style={{ fontFamily: "var(--font-type)", color: "var(--text-forge-mute)", fontSize: 9.5, letterSpacing: ".16em", marginTop: 5, whiteSpace: "nowrap" }}>IMAGE FORGE</div>
          </div>
        </div>
        <div style={{ height: 1, background: "var(--rule-gold)", opacity: 0.5, margin: "4px 14px 8px" }} />

        <div style={navGroupLabel}>THE JOB</div>
        {NAV_ITEMS.map(({ icon: Icon, label, active }) => (
          <div key={label} style={active ? navItemActive : navItemInactive}>
            <Icon size={18} />
            {label}
          </div>
        ))}

        <div style={{ ...navGroupLabel, padding: "16px 20px 4px" }}>THE HOUSE</div>
        {HOUSE_ITEMS.map(({ icon: Icon, label }) => (
          <div key={label} style={navItemInactive}>
            <Icon size={18} />
            {label}
          </div>
        ))}

        <div style={{ marginTop: "auto", padding: 14, borderTop: "1px solid var(--border-forge)", display: "flex", alignItems: "center", gap: 11 }}>
          <span style={{
            width: 36, height: 36, borderRadius: "50%", background: "var(--midnight-700)",
            border: "1.5px solid var(--gold-700)", color: "var(--gold-400)", fontFamily: "var(--font-display)",
            fontSize: 17, display: "flex", alignItems: "center", justifyContent: "center",
          }}>08</span>
          <div style={{ lineHeight: 1.25 }}>
            <div style={{ fontFamily: "var(--font-body)", color: "var(--text-forge-cream)", fontSize: 13 }}>Bojangles</div>
            <div style={{ display: "flex", alignItems: "center", gap: 5, fontFamily: "var(--font-type)", color: lineSecure ? "var(--absinthe)" : "var(--ember-400)", fontSize: 9, letterSpacing: ".1em" }}>
              <span style={{ width: 6, height: 6, borderRadius: "50%", background: lineSecure ? "var(--absinthe)" : "var(--ember-400)", boxShadow: lineSecure ? "0 0 7px var(--absinthe)" : "0 0 7px var(--ember-400)" }} />
              {lineSecure ? "LINE SECURE" : "LINE DOWN"}
            </div>
          </div>
        </div>
      </aside>

      {/* MAIN */}
      <div style={{
        position: "relative",
        backgroundColor: "var(--surface-forge-base)",
        backgroundImage: "linear-gradient(180deg, rgba(10,10,11,.72), rgba(10,10,11,.88)), url('./bg_embers.png')",
        backgroundSize: "cover", backgroundPosition: "center",
        display: "flex", flexDirection: "column", overflow: "hidden",
      }}>
        {/* header */}
        <div style={{ display: "flex", alignItems: "flex-end", justifyContent: "space-between", padding: "18px 26px 14px", background: "var(--surface-forge-head)", borderBottom: "1px solid var(--border-forge)" }}>
          <div>
            <div style={{ fontFamily: "var(--font-type)", fontSize: 10, letterSpacing: ".2em", textTransform: "uppercase", color: "var(--ember-400)" }}>Assignment · live</div>
            <h2 style={{ fontFamily: "var(--font-display)", color: "var(--gold-500)", fontSize: 34, lineHeight: 1, margin: "3px 0 0", fontWeight: 400 }}>The Hunt</h2>
          </div>
          <div style={{ display: "flex", alignItems: "center", gap: 14 }}>
            <span style={{ display: "inline-flex", alignItems: "center", gap: 7, fontFamily: "var(--font-type)", fontSize: 10, letterSpacing: ".1em", textTransform: "uppercase", color: "var(--text-forge-mute)" }}>
              <span style={{ width: 7, height: 7, borderRadius: "50%", background: "var(--flame-500)", boxShadow: "0 0 8px var(--flame-500)" }} />
              {running ? "machine hot" : "machine warm"}
            </span>
            <Button variant="ghost" size="sm" onClick={() => setResetKey((k) => k + 1)}>New haul</Button>
          </div>
        </div>

        {/* pipeline rail */}
        <div style={{ display: "flex", gap: 8, padding: "12px 26px", borderBottom: "1px solid var(--border-forge)" }}>
          <PipelineCell state="done" n="✓" label="THE MARK" />
          <PipelineCell state="active" n="2" label="THE HAUL" />
          <PipelineCell state="pending" n="3" label="INSPECT" />
          <PipelineCell state="pending" n="4" label="CULL" />
        </div>

        {/* body */}
        <div style={{ flex: 1, overflow: "auto", padding: "18px 26px 22px", display: "grid", gridTemplateColumns: "1.5fr 1fr", gridTemplateRows: "auto 1fr", gap: 18, minHeight: 0 }}>
          <div style={{ gridColumn: 1, gridRow: "1 / span 2", minHeight: 0 }}>
            <SearchPanel key={resetKey} />
          </div>
          <div style={{ gridColumn: 2, gridRow: 1 }}>
            <PageScrapePanel />
          </div>
          <div style={{ gridColumn: 2, gridRow: 2, display: "flex", flexDirection: "column", gap: 14, minHeight: 0 }}>
            <ProgressLog />
            <CurationGrid />
          </div>
        </div>
      </div>
    </div>
  );
}

function PipelineCell({ state, n, label }: { state: "done" | "active" | "pending"; n: string; label: string }) {
  const box: React.CSSProperties = {
    flex: 1, display: "flex", alignItems: "center", gap: 9, padding: "9px 12px", borderRadius: "var(--radius-forge-md)",
    background: state === "done" ? "var(--surface-forge-lit)" : "var(--surface-forge-well)",
    border: state === "pending" ? "1px solid var(--border-forge)" : "1px solid var(--gold-700)",
  };
  const circle: React.CSSProperties = {
    width: 20, height: 20, borderRadius: "50%", fontSize: 11, fontFamily: "var(--font-type)",
    display: "flex", alignItems: "center", justifyContent: "center",
    ...(state === "done"
      ? { background: "var(--gold-500)", color: "#1a1206", fontWeight: 800 }
      : state === "active"
      ? { background: "var(--surface-forge-card)", color: "var(--flame-500)", border: "1px solid var(--flame-500)" }
      : { background: "var(--surface-forge-card)", color: "var(--forge-idle)", border: "1px solid var(--border-forge)" }),
  };
  const text: React.CSSProperties = {
    fontFamily: "var(--font-type)", fontSize: 11, letterSpacing: ".08em",
    color: state === "done" ? "var(--gold-400)" : state === "active" ? "var(--text-forge-cream)" : "var(--text-forge-mute)",
  };
  return (
    <div style={box}>
      <span style={circle}>{n}</span>
      <span style={text}>{label}</span>
    </div>
  );
}

const navGroupLabel: React.CSSProperties = {
  fontFamily: "var(--font-type)", fontSize: 9.5, letterSpacing: ".2em", color: "var(--text-forge-faint)", padding: "8px 20px 4px",
};
const navItemBase: React.CSSProperties = {
  display: "flex", alignItems: "center", gap: 11, height: 44, padding: "0 20px",
  fontFamily: "var(--font-type)", fontSize: 12.5, letterSpacing: ".06em", borderLeft: "3px solid transparent",
};
const navItemActive: React.CSSProperties = { ...navItemBase, background: "var(--surface-forge-lit)", color: "var(--gold-400)", borderLeft: "3px solid var(--gold-500)" };
const navItemInactive: React.CSSProperties = { ...navItemBase, color: "var(--text-forge-mute)" };
