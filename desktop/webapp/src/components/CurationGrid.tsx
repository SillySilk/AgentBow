import { useEffect, useState, useCallback } from "react";
import { RotateCw } from "lucide-react";
import { useStore } from "../store";
import type { ImageItem, Slot } from "../api";
import { listImages, listSlots, thumbUrl, deleteImages, dedupe, openFolder } from "../api";
import Button from "./ui/Button";

function slotName(path: string): string {
  // Last path segment (handles both \ and /).
  const seg = path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? path;
  return seg;
}

function slotLabel(s: Slot): string {
  return /^\d+$/.test(s.name) ? `VAULT ${s.name} · ${s.count}` : `${s.name.toUpperCase()} · ${s.count}`;
}

export default function CurationGrid() {
  const baseDir = useStore((s) => s.lastDestDir);
  const dir = useStore((s) => s.workingSlotDir);
  const setWorkingSlot = useStore((s) => s.setWorkingSlot);
  const finished = useStore((s) => s.scrape.finished);
  const [items, setItems] = useState<ImageItem[]>([]);
  const [slots, setSlots] = useState<Slot[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [note, setNote] = useState("");

  const refresh = useCallback(async () => {
    const next = dir ? await listImages(dir) : [];
    setItems(next);
    setSelected(new Set());
  }, [dir]);

  const refreshSlots = useCallback(async () => {
    if (!baseDir) return;
    const next = await listSlots(baseDir);
    setSlots(next);
  }, [baseDir]);

  // Reload the previewed slot whenever it changes. State is set from the promise
  // callback, never synchronously in the effect (react-hooks/set-state-in-effect).
  useEffect(() => { Promise.resolve().then(refresh); }, [refresh]);
  // Reload images + the slot list when a scrape finishes.
  useEffect(() => {
    if (finished) Promise.resolve().then(() => { refresh(); refreshSlots(); });
  }, [finished, refresh, refreshSlots]);
  // Populate the slot list on first mount once we know the base dir.
  useEffect(() => { Promise.resolve().then(refreshSlots); }, [refreshSlots]);

  if (!baseDir) return null;

  const toggle = (p: string) => setSelected((s) => {
    const n = new Set(s);
    if (n.has(p)) n.delete(p); else n.add(p);
    return n;
  });

  const onDelete = async () => {
    if (selected.size === 0) return;
    const res = await deleteImages([...selected]);
    setNote(`Burned ${res.deleted}${res.errors ? `, ${res.errors} errors` : ""}`);
    await refresh();
    await refreshSlots();
  };
  const onDedupe = async () => { setNote(await dedupe(dir, true)); await refresh(); await refreshSlots(); };

  const active = dir ? slotName(dir) : "";

  return (
    <div style={{ flex: 1, background: "var(--surface-forge-card)", border: "1px solid var(--border-forge)", borderRadius: "var(--radius-forge-lg)", padding: "12px 14px", display: "flex", flexDirection: "column", minHeight: 0 }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: 10, flexWrap: "wrap", gap: 6 }}>
        <span style={{ fontFamily: "var(--font-type)", color: "var(--gold-500)", fontSize: 11, letterSpacing: ".16em" }}>THE LINEUP</span>
        <div style={{ display: "flex", gap: 5, flexWrap: "wrap", alignItems: "center" }}>
          {slots.length === 0 && <span style={{ fontFamily: "var(--font-type)", fontSize: 9, color: "var(--text-forge-mute)" }}>no vaults yet</span>}
          {slots.map((s) => {
            const isActive = s.name === active;
            return (
              <button key={s.path} onClick={() => setWorkingSlot(s.path)} style={{
                fontFamily: "var(--font-type)", fontSize: 9, letterSpacing: ".06em", padding: "3px 8px", borderRadius: 99, cursor: "pointer",
                color: isActive ? "#1a1206" : "var(--text-forge-mute)",
                background: isActive ? "var(--gold-500)" : "var(--surface-forge-well)",
                border: isActive ? "none" : "1px solid var(--border-forge)",
              }}>
                {slotLabel(s)}
              </button>
            );
          })}
          <button onClick={refreshSlots} title="Refresh vaults" style={{ background: "transparent", border: "none", color: "var(--text-forge-mute)", cursor: "pointer", display: "flex", padding: 3 }}>
            <RotateCw size={13} />
          </button>
        </div>
      </div>

      {dir && items.length > 0 ? (
        <>
          <div style={{ flex: 1, display: "grid", gridTemplateColumns: "repeat(5,1fr)", gridAutoRows: "minmax(52px,1fr)", gap: 6, overflow: "auto", minHeight: 0 }}>
            {items.map((it) => {
              const sel = selected.has(it.path);
              return (
                <div key={it.path} onClick={() => toggle(it.path)}
                  style={{ borderRadius: 4, overflow: "hidden", cursor: "pointer", border: `${sel ? 2 : 1}px solid ${sel ? "var(--gold-500)" : "var(--border-forge)"}` }}>
                  <img src={thumbUrl(it.path)} alt={it.name} loading="lazy"
                    style={{ width: "100%", height: "100%", objectFit: "cover", display: "block", opacity: sel ? 0.7 : 1 }} />
                </div>
              );
            })}
          </div>
          <div style={{ display: "flex", gap: 6, marginTop: 10, flexWrap: "wrap", alignItems: "center" }}>
            <Button variant="danger" size="sm" disabled={selected.size === 0} onClick={onDelete}>Burn ({selected.size})</Button>
            <Button variant="ghost" size="sm" onClick={onDedupe}>Cull doubles</Button>
            <Button variant="ghost" size="sm" onClick={() => openFolder(dir)}>Open vault</Button>
            <Button variant="ghost" size="sm" onClick={refresh}>Refresh</Button>
            {note && <span style={{ fontFamily: "var(--font-type)", fontSize: 10, color: "var(--text-forge-mute)" }}>{note}</span>}
          </div>
        </>
      ) : (
        dir && <span style={{ fontFamily: "var(--font-type)", fontSize: 11, color: "var(--text-forge-mute)" }}>Vault {active} is empty.</span>
      )}
    </div>
  );
}
