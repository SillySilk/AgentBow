import { create } from "zustand";

export type ScrapeEventMsg =
  | { type: "scrape_event"; kind: "phase"; label: string }
  | { type: "scrape_event"; kind: "source"; source: string; count: number; error: string | null }
  | { type: "scrape_event"; kind: "candidates"; total: number; filtered: number }
  | { type: "scrape_event"; kind: "downloaded"; done: number; target: number; path: string }
  | { type: "scrape_event"; kind: "failed"; url: string; reason: string }
  | { type: "scrape_event"; kind: "done"; downloaded: string[]; log_note: string }
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
    case "downloaded": return { ...s, done: m.done, target: m.target, downloaded: [...s.downloaded, m.path] };
    case "failed": return { ...s, log: [...s.log, `failed: ${m.reason}`] };
    case "done": return { ...s, running: false, finished: true, log: [...s.log, m.log_note] };
    case "error": return { ...s, running: false, finished: true, error: m.message, log: [...s.log, "ERROR: " + m.message] };
    default: return s;
  }
}

interface Store {
  status: string;
  scrape: ScrapeState;
  lastDestDir: string;
  connect: () => void;
  startScrape: (a: { query: string; count: number; destDir: string; sources: string[] }) => void;
  _ws?: WebSocket;
}

export const useStore = create<Store>((set, get) => ({
  status: "connecting…",
  scrape: initialScrapeState(),
  lastDestDir: "",
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
        else if (m.type === "scrape_event") set((st) => ({ scrape: applyEvent(st.scrape, m) }));
      };
      ws.onclose = () => set({ status: "disconnected" });
      ws.onerror = () => set({ status: "error" });
      set({ _ws: ws });
    }).catch(() => set({ status: "config unavailable" }));
  },
  startScrape: (a: { query: string; count: number; destDir: string; sources: string[] }) => {
    const ws = get()._ws;
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    set({ scrape: { ...initialScrapeState(), running: true, target: a.count }, lastDestDir: a.destDir });
    ws.send(JSON.stringify({ type: "scrape_request", query: a.query, count: a.count, dest_dir: a.destDir, sources: a.sources }));
  },
}));
