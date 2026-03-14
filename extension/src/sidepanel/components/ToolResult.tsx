import { useState } from "react";
import { ToolCard } from "../store/useChat";

interface Props {
  tool: ToolCard;
}

export function ToolResult({ tool }: Props) {
  const [open, setOpen] = useState(false);

  const inputStr =
    typeof tool.input === "string"
      ? tool.input
      : JSON.stringify(tool.input, null, 2);

  return (
    <div className={`rounded border text-xs my-1 ${tool.is_error ? "border-red-500/40" : "border-border"}`}>
      <button
        onClick={() => setOpen((o) => !o)}
        className="w-full flex items-center gap-2 px-3 py-2 hover:bg-white/5 transition-colors text-left"
      >
        <span className={tool.done
          ? tool.is_error ? "text-red-400" : "text-green-400"
          : "text-yellow-400 animate-pulse"
        }>
          {tool.done ? (tool.is_error ? "✗" : "✓") : "⟳"}
        </span>
        <span className="font-mono font-bold text-white">{tool.tool_name}</span>
        <span className="text-muted/60 ml-auto">{open ? "▲" : "▼"}</span>
      </button>

      {open && (
        <div className="border-t border-border px-3 py-2 space-y-2">
          <div>
            <div className="text-muted/60 mb-1">Input</div>
            <pre className="font-mono text-muted whitespace-pre-wrap break-all max-h-32 overflow-y-auto">
              {inputStr}
            </pre>
          </div>
          {tool.done && tool.output && (
            <div>
              <div className={`mb-1 ${tool.is_error ? "text-red-400" : "text-muted/60"}`}>
                {tool.is_error ? "Error" : "Output"}
              </div>
              <pre className="font-mono text-muted whitespace-pre-wrap break-all max-h-48 overflow-y-auto">
                {tool.output}
              </pre>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
