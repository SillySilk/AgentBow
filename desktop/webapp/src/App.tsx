import { useEffect } from "react";
import { useStore } from "./store";
import SearchPanel from "./components/SearchPanel";
import ProgressLog from "./components/ProgressLog";

export default function App() {
  const connect = useStore((s) => s.connect);
  const status = useStore((s) => s.status);
  useEffect(() => { connect(); }, [connect]);
  return (
    <div style={{ fontFamily: "system-ui", padding: 24, background: "#1a1a2e", color: "#a8b2d8", minHeight: "100vh" }}>
      <h1 style={{ color: "#e94560", marginTop: 0 }}>Bow Image Studio</h1>
      <p style={{ marginTop: -8, fontSize: 13 }}>Backend: {status}</p>
      <SearchPanel />
      <ProgressLog />
    </div>
  );
}
