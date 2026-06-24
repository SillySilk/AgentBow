import { useEffect, useState, useCallback } from "react";
import { useStore } from "../store";
import type { ImageItem, Slot } from "../api";
import { listImages, listSlots, thumbUrl, deleteImages, dedupe, openFolder } from "../api";

const tool: React.CSSProperties = { background: "#0f3460", color: "#a8b2d8", border: "1px solid #2a2a4a", borderRadius: 6, padding: "6px 10px", cursor: "pointer", fontSize: 12 };

function slotName(path: string): string {
  // Last path segment (handles both \ and /).
  const seg = path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? path;
  return seg;
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
    if (!dir) { setItems([]); setSelected(new Set()); return; }
    setItems(await listImages(dir));
    setSelected(new Set());
  }, [dir]);

  const refreshSlots = useCallback(async () => {
    if (!baseDir) return;
    setSlots(await listSlots(baseDir));
  }, [baseDir]);

  // Reload the previewed slot whenever it changes.
  useEffect(() => { refresh(); }, [refresh]);
  // Reload images + the slot list when a scrape finishes.
  useEffect(() => { if (finished) { refresh(); refreshSlots(); } }, [finished, refresh, refreshSlots]);
  // Populate the slot list on first mount once we know the base dir.
  useEffect(() => { refreshSlots(); }, [refreshSlots]);

  if (!baseDir) return null;

  const toggle = (p: string) => setSelected((s) => { const n = new Set(s); n.has(p) ? n.delete(p) : n.add(p); return n; });

  const onDelete = async () => {
    if (selected.size === 0) return;
    const res = await deleteImages([...selected]);
    setNote(`Deleted ${res.deleted}${res.errors ? `, ${res.errors} errors` : ""}`);
    await refresh();
    await refreshSlots();
  };
  const onDedupe = async () => { setNote(await dedupe(dir, true)); await refresh(); await refreshSlots(); };

  const active = dir ? slotName(dir) : "";

  return (
    <div style={{ marginTop: 20 }}>
      {/* Slot switcher — each numbered set is a working slot you can load into the preview. */}
      <div style={{ display: "flex", gap: 6, alignItems: "center", flexWrap: "wrap", marginBottom: 10 }}>
        <span style={{ color: "#8893b8", fontSize: 12 }}>Working slot:</span>
        {slots.length === 0 && <span style={{ color: "#8893b8", fontSize: 12 }}>none yet</span>}
        {slots.map((s) => {
          const isActive = s.name === active;
          return (
            <button key={s.path} onClick={() => setWorkingSlot(s.path)}
              style={{
                ...tool,
                background: isActive ? "#e94560" : "#0f3460",
                color: isActive ? "white" : "#a8b2d8",
                fontWeight: isActive ? 600 : 400,
              }}>
              {s.name} <span style={{ opacity: 0.7 }}>({s.count})</span>
            </button>
          );
        })}
        <button onClick={refreshSlots} style={tool}>↻</button>
      </div>

      {dir && items.length > 0 ? (
        <>
          <div style={{ display: "flex", gap: 8, alignItems: "center", marginBottom: 8 }}>
            <strong style={{ color: "#a8b2d8" }}>Slot {active} — {items.length} images</strong>
            <button onClick={onDelete} disabled={selected.size === 0} style={tool}>Delete selected ({selected.size})</button>
            <button onClick={onDedupe} style={tool}>Remove duplicates</button>
            <button onClick={() => openFolder(dir)} style={tool}>Open folder</button>
            <button onClick={refresh} style={tool}>Refresh</button>
            {note && <span style={{ color: "#8893b8", fontSize: 12 }}>{note}</span>}
          </div>
          <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(140px, 1fr))", gap: 8 }}>
            {items.map((it) => {
              const sel = selected.has(it.path);
              return (
                <div key={it.path} onClick={() => toggle(it.path)}
                  style={{ border: `2px solid ${sel ? "#e94560" : "#2a2a4a"}`, borderRadius: 8, overflow: "hidden", cursor: "pointer", background: "#16213e" }}>
                  <img src={thumbUrl(it.path)} alt={it.name} loading="lazy"
                    style={{ width: "100%", height: 120, objectFit: "cover", display: "block", opacity: sel ? 0.7 : 1 }} />
                  <div style={{ fontSize: 10, color: "#8893b8", padding: "2px 4px", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{it.name}</div>
                </div>
              );
            })}
          </div>
        </>
      ) : (
        dir && <span style={{ color: "#8893b8", fontSize: 12 }}>Slot {active} is empty.</span>
      )}
    </div>
  );
}
