import { useEffect, useState } from "react";
import { useStore } from "../store";
import type { Candidate, Recipe } from "../store";
import Button from "./ui/Button";

function domainOf(u: string): string {
  try { return new URL(u).hostname.replace(/^www\./, "").toLowerCase(); } catch { return ""; }
}

/** Guided grab: Case it → click a thumbnail → (if it links out) click the full
 *  image on the detail page → Grab the batch. Optionally save/load a playbook. */
export default function CasePanel({ url, destDir, count, scrolls }: { url: string; destDir: string; count: number; scrolls: number }) {
  const cs = useStore((s) => s.caseState);
  const caseExtract = useStore((s) => s.caseExtract);
  const caseOpenDetail = useStore((s) => s.caseOpenDetail);
  const caseGeneralize = useStore((s) => s.caseGeneralize);
  const caseRun = useStore((s) => s.caseRun);
  const playbookSave = useStore((s) => s.playbookSave);
  const playbookList = useStore((s) => s.playbookList);
  const status = useStore((s) => s.status);
  const running = useStore((s) => s.scrape.running);
  const ready = status === "connected";

  const domain = domainOf(url);
  const [pendingExample, setPendingExample] = useState<number | null>(null);
  const [loaded, setLoaded] = useState<Recipe | null>(null);

  // Fetch saved playbooks whenever the target domain changes.
  useEffect(() => { if (ready && domain) playbookList(domain); }, [ready, domain, playbookList]);

  // The recipe available to Grab/Save: a freshly-cased one wins over a loaded one.
  const activeRecipe = cs.stage === "recipe" ? cs.recipe : loaded;
  const gridUrl = cs.stage === "recipe" ? cs.gridUrl : url;

  const onTile = (c: Candidate) => {
    if (cs.stage === "grid") {
      if (c.href) { setPendingExample(c.id); caseOpenDetail(c.href); }
      else caseGeneralize(c.id, undefined, scrolls);
    } else if (cs.stage === "detail" && pendingExample != null) {
      caseGeneralize(pendingExample, c.id, scrolls);
      setPendingExample(null);
    }
  };

  return (
    <div style={{ marginTop: 12, borderTop: "1px solid var(--border-forge)", paddingTop: 10 }}>
      <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 8, flexWrap: "wrap" }}>
        <span style={{ fontFamily: "var(--font-type)", fontSize: 9.5, letterSpacing: ".16em", color: "var(--text-forge-mute)" }}>TEACH A GALLERY</span>
        <Button variant="ember" size="sm" disabled={!ready} onClick={() => caseExtract()}>Case it</Button>
        {cs.playbooks.length > 0 && (
          <select
            className="forge-input"
            style={{ fontSize: 12, padding: "4px 6px" }}
            value=""
            onChange={(e) => { const r = cs.playbooks[Number(e.target.value)]; if (r) { setLoaded(r); } }}
          >
            <option value="">Load playbook…</option>
            {cs.playbooks.map((p, i) => <option key={i} value={i}>{p.grid_selector.slice(0, 40)}</option>)}
          </select>
        )}
      </div>

      {cs.stage === "detail" && (
        <div style={{ fontFamily: "var(--font-type)", fontSize: 10, color: "var(--absinthe)", marginBottom: 6 }}>
          Click the full-size image on this detail page.
        </div>
      )}

      {activeRecipe && (
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 8, flexWrap: "wrap" }}>
          <span style={{ fontFamily: "var(--font-type)", fontSize: 10, color: "var(--text-forge-mute)" }}>
            {cs.stage === "recipe" ? `Matched ${cs.matched} of ${cs.total}.` : `Loaded: ${activeRecipe.domain}.`}
          </span>
          <Button variant="ghost" size="sm" disabled={!ready || running || !destDir.trim() || !gridUrl.trim()}
            onClick={() => caseRun({ ...activeRecipe, scrolls }, gridUrl, count, destDir)}>
            {running ? "Working…" : `Grab · ${count}`}
          </Button>
          <Button variant="ghost" size="sm" disabled={!ready} onClick={() => playbookSave({ ...activeRecipe, scrolls })}>Save playbook</Button>
        </div>
      )}

      {(cs.stage === "grid" || cs.stage === "detail") && (
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(64px, 1fr))", gap: 6, maxHeight: 240, overflow: "auto" }}>
          {cs.candidates.map((c) => (
            <img key={c.id} src={c.preview_url} alt="" title={c.selector} loading="lazy"
              onClick={() => onTile(c)}
              style={{ width: "100%", height: 64, objectFit: "cover", cursor: "pointer", borderRadius: 4, border: "1px solid var(--gold-700)" }} />
          ))}
        </div>
      )}
    </div>
  );
}
