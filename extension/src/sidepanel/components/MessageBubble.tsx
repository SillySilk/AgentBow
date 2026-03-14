import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { ChatMessage } from "../store/useChat";
import { ToolResult } from "./ToolResult";

interface Props {
  message: ChatMessage;
}

export function MessageBubble({ message }: Props) {
  const isUser = message.role === "user";

  return (
    <div className={`flex ${isUser ? "justify-end" : "justify-start"} mb-3`}>
      <div
        className={`max-w-[85%] rounded-xl px-4 py-3 text-sm ${
          isUser
            ? "bg-user text-white rounded-br-sm"
            : "bg-surface border border-border text-muted rounded-bl-sm"
        }`}
      >
        {isUser ? (
          <p className="whitespace-pre-wrap">{message.text}</p>
        ) : (
          <div className="prose prose-invert prose-sm max-w-none">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>
              {message.text}
            </ReactMarkdown>
            {message.streaming && !message.text && (
              <span className="inline-block w-2 h-4 bg-accent animate-pulse rounded-sm" />
            )}
            {message.streaming && message.text && (
              <span className="inline-block w-2 h-4 bg-muted/50 animate-pulse rounded-sm ml-0.5" />
            )}
          </div>
        )}

        {message.tools.length > 0 && (
          <div className="mt-2 space-y-1">
            {message.tools.map((t) => (
              <ToolResult key={t.tool_use_id} tool={t} />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
