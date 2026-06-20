import { useEffect, useState } from "react";

export default function App() {
  const [status, setStatus] = useState("connecting…");
  const [wsPort, setWsPort] = useState<number | "unavailable" | null>(null);

  useEffect(() => {
    let ws: WebSocket | null = null;

    fetch("/api/config")
      .then((r) => {
        if (!r.ok) throw new Error(`config fetch failed: ${r.status}`);
        return r.json();
      })
      .then((cfg) => {
        setWsPort(cfg.ws_port ?? null);

        const token: string = cfg.token;
        if (!token) {
          setStatus("auth error: no token in /api/config");
          return;
        }

        const wsUrl = `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`;
        ws = new WebSocket(wsUrl);
        ws.onopen = () => {
          ws!.send(JSON.stringify({ type: "auth", token, session_id: crypto.randomUUID() }));
        };
        ws.onmessage = (e) => {
          const m = JSON.parse(e.data);
          if (m.type === "auth_ok") setStatus("connected");
          else if (m.type === "auth_error") setStatus("auth error: " + (m.message ?? ""));
        };
        ws.onclose = () => setStatus("disconnected");
        ws.onerror = () => setStatus("error");
      })
      .catch((err) => {
        setWsPort("unavailable");
        setStatus("config fetch error: " + err.message);
      });

    return () => {
      if (ws) ws.close();
    };
  }, []);

  return (
    <div style={{ fontFamily: "system-ui", padding: 24, background: "#1a1a2e", color: "#a8b2d8", minHeight: "100vh" }}>
      <h1 style={{ color: "#e94560" }}>Bow Image Studio</h1>
      <p>Backend: {status}</p>
      <p>Server port: {wsPort === null ? "loading…" : wsPort === "unavailable" ? "config unavailable" : wsPort}</p>
    </div>
  );
}
