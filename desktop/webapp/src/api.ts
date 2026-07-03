export interface ImageItem { name: string; path: string; bytes: number }
export interface Slot { name: string; path: string; count: number }

export interface EngineModelRef {
  name: string;
  path: string;
  size_bytes: number;
  quant: string | null;
  mmproj: string | null;
}
export interface EngineStatus {
  state: "stopped" | "starting" | "ready" | "failed";
  error: string | null;
  base_url: string | null;
  vision: boolean;
  model: EngineModelRef | null;
}
export interface EngineModel {
  name: string;
  path: string;
  size_bytes: number;
  quant: string | null;
  vision: boolean;
  loadable: boolean;
}
export interface ModelsList { dir: string; models: EngineModel[] }

const STOPPED_STATUS: EngineStatus = { state: "stopped", error: null, base_url: null, vision: false, model: null };

/** 400 responses from engine endpoints carry JSON `{error}`; parse defensively
 * since other endpoints in this codebase return plain text on failure. */
async function readErrorBody(r: Response): Promise<string> {
  const text = await r.text();
  try {
    const j = JSON.parse(text) as { error?: unknown };
    if (j && typeof j.error === "string") return j.error;
  } catch { /* not JSON, fall through to raw text */ }
  return text || `request failed (${r.status})`;
}

export async function engineStatus(): Promise<EngineStatus> {
  const r = await fetch("/api/engine");
  if (!r.ok) { console.error("engineStatus failed", r.status); return STOPPED_STATUS; }
  return r.json();
}
export async function listModels(): Promise<ModelsList> {
  const r = await fetch("/api/models");
  if (!r.ok) { console.error("listModels failed", r.status); return { dir: "", models: [] }; }
  return r.json();
}
export async function loadModel(path: string): Promise<{ ok?: boolean; error?: string }> {
  const r = await fetch("/api/engine/load", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ path }) });
  if (!r.ok) {
    const error = await readErrorBody(r);
    console.error("loadModel failed", r.status, error);
    return { error };
  }
  return r.json();
}
export async function stopEngine(): Promise<void> {
  const r = await fetch("/api/engine/stop", { method: "POST" });
  if (!r.ok) console.error("stopEngine failed", r.status);
}
export async function setModelsDir(dir: string): Promise<ModelsList> {
  const r = await fetch("/api/engine/models-dir", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ dir }) });
  if (!r.ok) {
    console.error("setModelsDir failed", r.status, await readErrorBody(r));
    return { dir: "", models: [] };
  }
  return r.json();
}

export async function listSlots(dir: string): Promise<Slot[]> {
  const r = await fetch(`/api/slots?dir=${encodeURIComponent(dir)}`);
  if (!r.ok) { console.error("listSlots failed", r.status); return []; }
  return (await r.json()).slots as Slot[];
}

export function thumbUrl(path: string, w = 256): string {
  return `/api/thumb?path=${encodeURIComponent(path)}&w=${w}`;
}
export async function listImages(dir: string): Promise<ImageItem[]> {
  const r = await fetch(`/api/images?dir=${encodeURIComponent(dir)}`);
  if (!r.ok) { console.error("listImages failed", r.status); return []; }
  return (await r.json()).images as ImageItem[];
}
export async function deleteImages(paths: string[]): Promise<{ deleted: number; errors: number }> {
  const r = await fetch("/api/images/delete", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ paths }) });
  if (!r.ok) { console.error("delete failed", r.status); return { deleted: 0, errors: paths.length }; }
  return r.json();
}
export async function dedupe(dir: string, apply: boolean): Promise<string> {
  const r = await fetch("/api/curate/dedupe", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ dir, apply }) });
  if (!r.ok) { console.error("dedupe failed", r.status); return ""; }
  return (await r.json()).report ?? "";
}
export async function openFolder(path: string): Promise<void> {
  await fetch("/api/open-folder", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ path }) });
}
