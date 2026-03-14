import { useEffect, useRef, useState } from "react";
import { useChat } from "../store/useChat";
import { useConnection } from "../store/useConnection";
import { MessageBubble } from "./MessageBubble";
import { v4 as uuidv4 } from "uuid";

export function ChatView() {
  const { messages, isLoading, startAssistantMessage } = useChat();
  const { status, sendUserMessage, sendInterrupt } = useConnection();
  const [input, setInput] = useState("");
  const bottomRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  const handleSend = async () => {
    const content = input.trim();
    if (!content || status !== "connected" || isLoading) return;

    setInput("");
    const userMsgId = uuidv4();

    useChat.getState().addUserMessage(content, userMsgId);

    const messageId = await sendUserMessage(content);
    if (messageId) {
      startAssistantMessage(messageId);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  const canSend = status === "connected" && input.trim().length > 0 && !isLoading;

  return (
    <div className="flex flex-col flex-1 min-h-0">
      {/* Messages */}
      <div className="flex-1 overflow-y-auto px-3 py-2">
        {messages.length === 0 && (
          <div className="text-center text-muted/40 text-sm mt-8">
            <p>Ask Bow anything.</p>
            <p className="text-xs mt-1">It can read files, run PowerShell, search the web.</p>
          </div>
        )}
        {messages.map((msg) => (
          <MessageBubble key={msg.id} message={msg} />
        ))}
        <div ref={bottomRef} />
      </div>

      {/* Input */}
      <div className="border-t border-border p-3 space-y-2">
        <div className="flex gap-2">
          <textarea
            ref={textareaRef}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={
              status === "connected"
                ? "Message Bow… (Enter to send, Shift+Enter for newline)"
                : "Waiting for connection…"
            }
            disabled={status !== "connected"}
            rows={3}
            className="flex-1 bg-surface border border-border rounded-lg px-3 py-2 text-sm text-white placeholder-muted/40 resize-none focus:outline-none focus:border-accent/50 disabled:opacity-50"
          />
        </div>
        <div className="flex justify-between items-center">
          {isLoading ? (
            <button
              onClick={sendInterrupt}
              className="px-4 py-1.5 bg-accent/20 text-accent border border-accent/40 rounded-lg text-xs hover:bg-accent/30 transition-colors"
            >
              Stop
            </button>
          ) : (
            <button
              onClick={handleSend}
              disabled={!canSend}
              className="px-4 py-1.5 bg-accent text-white rounded-lg text-xs hover:bg-accent/90 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
            >
              Send
            </button>
          )}
          <button
            onClick={() => useChat.getState().clearHistory()}
            className="text-xs text-muted/40 hover:text-muted transition-colors"
          >
            Clear
          </button>
        </div>
      </div>
    </div>
  );
}
