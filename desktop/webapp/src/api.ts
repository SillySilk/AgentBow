export interface ImageItem { name: string; path: string; bytes: number }
export interface Slot { name: string; path: string; count: number }

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
