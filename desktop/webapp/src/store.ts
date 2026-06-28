import { create } from "zustand";

export type ScrapeEventMsg =
  | { type: "scrape_event"; kind: "phase"; label: string }
  | { type: "scrape_event"; kind: "source"; source: string; count: number; error: string | null }
  | { type: "scrape_event"; kind: "candidates"; total: number; filtered: number }
  | { type: "scrape_event"; kind: "verifying"; url: string; done: number; target: number }
  | { type: "scrape_event"; kind: "downloaded"; done: number; target: number; path: string }
  | { type: "scrape_event"; kind: "failed"; url: string; reason: string }
  | { type: "scrape_event"; kind: "done"; downloaded: string[]; log_note: string; dest_dir: string }
  | { type: "scrape_event"; kind: "error"; message: string };

export interface ScrapeState {
  running: boolean;
  finished: boolean;
  phase: string;
  done: number;
  target: number;
  downloaded: string[];
  sources: { source: string; count: number; error: string | null }[];
  log: string[];
  error: string | null;
}

export function initialScrapeState(): ScrapeState {
  return { running: false, finished: false, phase: "", done: 0, target: 0, downloaded: [], sources: [], log: [], error: null };
}

export function applyEvent(s: ScrapeState, m: ScrapeEventMsg): ScrapeState {
  switch (m.kind) {
    case "phase": return { ...s, phase: m.label, log: [...s.log, m.label] };
    case "source": return { ...s, sources: [...s.sources, { source: m.source, count: m.count, error: m.error }],
                            log: [...s.log, `${m.source}: ${m.error ? "ERROR " + m.error : m.count + " URLs"}`] };
    case "candidates": return { ...s, log: [...s.log, `candidates: ${m.total} (filtered ${m.filtered})`] };
    case "verifying": return { ...s, phase: `Verifying image ${m.done + 1}/${m.target}…` };
    case "downloaded": return { ...s, phase: "Downloading", done: m.done, target: m.target, downloaded: [...s.downloaded, m.path] };
    case "failed": return { ...s, log: [...s.log, `failed: ${m.reason}`] };
    case "done": return { ...s, running: false, finished: true, log: [...s.log, m.log_note] };
    case "error": return { ...s, running: false, finished: true, error: m.message, log: [...s.log, "ERROR: " + m.message] };
    default: return s;
  }
}

export function isBrowserOpened(m: any): boolean { return m?.type === "browser_opened"; }

interface Store {
  status: string;
  scrape: ScrapeState;
  lastDestDir: string;
  /** Folder currently shown in the preview (the active "working slot"). */
  workingSlotDir: string;
  browserUrl?: string;
  connect: () => void;
  startScrape: (a: { query: string; count: number; destDir: string; sources: string[]; delayMs: number; verify: boolean; visionPrompt: string; bin: number | null; dedupe: boolean }) => void;
  setWorkingSlot: (dir: string) => void;
  openBrowser: (url: string) => void;
  pageScrape: (a: { count: number; destDir: string; scrolls: number }) => void;
  _ws?: WebSocket;
}

export const useStore = create<Store>((set, get) => ({
  status: "connecting…",
  scrape: initialScrapeState(),
  lastDestDir: "",
  workingSlotDir: "",
  connect: () => {
    fetch("/api/config").then(r => r.json()).then(cfg => {
      const token: string = cfg.token ?? "";
      if (!token) { set({ status: "no token in /api/config" }); return; }
      // Close previous socket atomically, immediately before opening the new one.
      const prev = get()._ws;
      if (prev) { try { prev.close(); } catch {} }
      const wsUrl = `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`;
      const ws = new WebSocket(wsUrl);
      ws.onopen = () => ws.send(JSON.stringify({ type: "auth", token, session_id: crypto.randomUUID() }));
      ws.onmessage = (e) => {
        const m = JSON.parse(e.data);
        if (m.type === "auth_ok") set({ status: "connected" });
        else if (m.type === "auth_error") set({ status: "auth error: " + (m.message ?? "") });
        else if (m.type === "scrape_event") set((st) => {
          const next: Partial<Store> = { scrape: applyEvent(st.scrape, m) };
          // When a scrape finishes, the freshly-filled slot becomes the working slot.
          if (m.kind === "done" && m.dest_dir) next.workingSlotDir = m.dest_dir;
          return next;
        });
        else if (isBrowserOpened(m)) set({ browserUrl: m.url });
      };
      ws.onclose = () => set({ status: "disconnected" });
      ws.onerror = () => set({ status: "error" });
      set({ _ws: ws });
    }).catch(() => set({ status: "config unavailable" }));
  },
  startScrape: (a: { query: string; count: number; destDir: string; sources: string[]; delayMs: number; verify: boolean; visionPrompt: string; bin: number | null; dedupe: boolean }) => {
    const ws = get()._ws;
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    set({ scrape: { ...initialScrapeState(), running: true, target: a.count }, lastDestDir: a.destDir });
    ws.send(JSON.stringify({
      type: "scrape_request", query: a.query, count: a.count, dest_dir: a.destDir, sources: a.sources,
      delay_ms: a.delayMs, verify: a.verify, vision_prompt: a.visionPrompt.trim() || null,
      bin: a.bin, dedupe: a.dedupe,
    }));
  },
  setWorkingSlot: (dir: string) => set({ workingSlotDir: dir }),
  openBrowser: (url: string) => {
    const ws = get()._ws;
    if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ type: "browser_open", url }));
  },
  pageScrape: ({ count, destDir, scrolls }: { count: number; destDir: string; scrolls: number }) => {
    const ws = get()._ws;
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    set({ scrape: { ...initialScrapeState(), running: true, target: count }, lastDestDir: destDir });
    ws.send(JSON.stringify({ type: "page_scrape_request", count, dest_dir: destDir, scrolls }));
  },
}));
