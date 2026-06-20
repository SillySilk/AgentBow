import { useEffect, useState } from "react";

export default function App() {
  const [status, setStatus] = useState("connecting…");
  useEffect(() => {
    const wsUrl = `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`;
    const ws = new WebSocket(wsUrl);
    ws.onopen = () => {
      ws.send(JSON.stringify({ type: "auth", token: "dev", session_id: crypto.randomUUID() }));
    };
    ws.onmessage = (e) => {
      const m = JSON.parse(e.data);
      if (m.type === "auth_ok") setStatus("connected");
      else if (m.type === "auth_error") setStatus("auth error: " + (m.message ?? ""));
    };
    ws.onclose = () => setStatus("disconnected");
    ws.onerror = () => setStatus("error");
    return () => ws.close();
  }, []);
  return (
    <div style={{ fontFamily: "system-ui", padding: 24, background: "#1a1a2e", color: "#a8b2d8", minHeight: "100vh" }}>
      <h1 style={{ color: "#e94560" }}>Bow Image Studio</h1>
      <p>Backend: {status}</p>
    </div>
  );
}
