import { useEffect, useState } from "react";
import { useConnection } from "./store/useConnection";
import { useChat } from "./store/useChat";
import { ConnectionStatus } from "./components/ConnectionStatus";
import { PageContext } from "./components/PageContext";
import { ChatView } from "./components/ChatView";

type Tab = "chat" | "settings";

export default function App() {
  const [activeTab, setActiveTab] = useState<Tab>("chat");
  const [tokenInput, setTokenInput] = useState("");
  const { setStatus, setCurrentPage } = useConnection();
  const { appendTextDelta, addToolStart, resolveToolResult, completeMessage, startAssistantMessage } = useChat();

  // Wire up chrome.runtime.onMessage → store updates
  useEffect(() => {
    // Keep service worker alive via persistent port connection
    const keepAlivePort = chrome.runtime.connect({ name: "bow-keepalive" });

    // Get initial connection status
    chrome.runtime.sendMessage({ type: "get_status" }, (resp) => {
      if (resp?.status) {
        setStatus(resp.status);
      }
    });

    const handler = (msg: Record<string, unknown>) => {
      switch (msg.type) {
        case "_connection_status":
          setStatus(msg.status as string, msg.message as string | undefined);
          break;

        case "_page_context_update":
          setCurrentPage({ url: msg.url as string, title: msg.title as string });
          break;

        case "text_delta": {
          const mid = msg.message_id as string;
          // If no assistant message exists yet for this id, create it
          const existing = useChat.getState().messages.find((m) => m.id === mid);
          if (!existing) {
            startAssistantMessage(mid);
          }
          appendTextDelta(mid, msg.delta as string);
          break;
        }

        case "tool_start":
          addToolStart(useChat.getState().activeMessageId || "", {
            tool_use_id: msg.tool_use_id as string,
            tool_name: msg.tool_name as string,
            input: msg.input,
            done: false,
          });
          break;

        case "tool_result":
          resolveToolResult(
            useChat.getState().activeMessageId || "",
            msg.tool_use_id as string,
            msg.output as string,
            msg.is_error as boolean
          );
          break;

        case "message_complete":
          completeMessage(useChat.getState().activeMessageId || "");
          break;

        case "error":
          completeMessage(useChat.getState().activeMessageId || "");
          break;
      }
    };

    chrome.runtime.onMessage.addListener(handler);
    return () => chrome.runtime.onMessage.removeListener(handler);
  }, []);

  const saveToken = () => {
    if (!tokenInput.trim()) return;
    chrome.runtime.sendMessage({ type: "set_token", token: tokenInput.trim() }, () => {
      setTokenInput("");
    });
  };

  return (
    <div className="flex flex-col h-screen bg-panel text-muted">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-border shrink-0">
        <div className="flex items-center gap-3">
          <span className="font-bold text-white text-lg">Bow</span>
          <ConnectionStatus />
        </div>
        <div className="flex gap-1">
          {(["chat", "settings"] as Tab[]).map((t) => (
            <button
              key={t}
              onClick={() => setActiveTab(t)}
              className={`px-3 py-1 rounded text-xs capitalize transition-colors ${
                activeTab === t
                  ? "bg-accent text-white"
                  : "text-muted hover:text-white"
              }`}
            >
              {t}
            </button>
          ))}
        </div>
      </div>

      {activeTab === "chat" ? (
        <>
          <div className="px-3 pt-2 shrink-0">
            <PageContext />
          </div>
          <ChatView />
        </>
      ) : (
        <div className="p-4 space-y-4">
          <div>
            <label className="block text-xs text-muted mb-2">
              BOW_SECRET token (from desktop/.env)
            </label>
            <input
              type="password"
              value={tokenInput}
              onChange={(e) => setTokenInput(e.target.value)}
              placeholder="Paste your BOW_SECRET here"
              className="w-full bg-surface border border-border rounded-lg px-3 py-2 text-sm text-white placeholder-muted/40 focus:outline-none focus:border-accent/50"
            />
          </div>
          <button
            onClick={saveToken}
            disabled={!tokenInput.trim()}
            className="w-full py-2 bg-accent text-white rounded-lg text-sm hover:bg-accent/90 disabled:opacity-40"
          >
            Save & Reconnect
          </button>
          <p className="text-xs text-muted/50">
            Token is stored locally in chrome.storage.local and never sent outside your machine.
          </p>
        </div>
      )}
    </div>
  );
}
