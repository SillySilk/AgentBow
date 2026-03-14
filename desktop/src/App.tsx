import { useState } from "react";

export default function App() {
  const [wsPort] = useState(9357);
  const [connections] = useState(0);

  return (
    <div className="min-h-screen bg-panel text-muted flex flex-col items-center justify-center p-6">
      <div className="text-center space-y-4">
        <h1 className="text-2xl font-bold text-white">Bow</h1>
        <p className="text-sm text-muted">AI Agent — Running</p>

        <div className="bg-surface border border-border rounded-lg p-4 space-y-2 text-sm">
          <div className="flex justify-between">
            <span className="text-muted">WebSocket</span>
            <span className="text-green-400">ws://127.0.0.1:{wsPort}</span>
          </div>
          <div className="flex justify-between">
            <span className="text-muted">Status</span>
            <span className="text-green-400">● Active</span>
          </div>
          <div className="flex justify-between">
            <span className="text-muted">Connections</span>
            <span className="text-white">{connections}</span>
          </div>
        </div>

        <p className="text-xs text-muted/60">
          Minimize to tray — open the Chrome extension to chat
        </p>
      </div>
    </div>
  );
}
