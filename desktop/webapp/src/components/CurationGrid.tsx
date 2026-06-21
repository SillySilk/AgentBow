import { useEffect, useState, useCallback } from "react";
import { useStore } from "../store";
import type { ImageItem } from "../api";
import { listImages, thumbUrl, deleteImages, dedupe, openFolder } from "../api";

export default function CurationGrid() {
  const dir = useStore((s) => s.lastDestDir);
  const finished = useStore((s) => s.scrape.finished);
  const [items, setItems] = useState<ImageItem[]>([]);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [note, setNote] = useState("");

  const refresh = useCallback(async () => {
    if (!dir) return;
    setItems(await listImages(dir));
    setSelected(new Set());
  }, [dir]);

  useEffect(() => { if (finished) refresh(); }, [finished, refresh]);

  if (!dir || items.length === 0) return null;

  const toggle = (p: string) => setSelected((s) => { const n = new Set(s); n.has(p) ? n.delete(p) : n.add(p); return n; });

  const onDelete = async () => {
    if (selected.size === 0) return;
    const res = await deleteImages([...selected]);
    setNote(`Deleted ${res.deleted}${res.errors ? `, ${res.errors} errors` : ""}`);
    refresh();
  };
  const onDedupe = async () => { setNote(await dedupe(dir, true)); refresh(); };

  return (
    <div style={{ marginTop: 20 }}>
      <div style={{ display: "flex", gap: 8, alignItems: "center", marginBottom: 8 }}>
        <strong style={{ color: "#a8b2d8" }}>{items.length} images</strong>
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
    </div>
  );
}
const tool: React.CSSProperties = { background: "#0f3460", color: "#a8b2d8", border: "1px solid #2a2a4a", borderRadius: 6, padding: "6px 10px", cursor: "pointer", fontSize: 12 };
