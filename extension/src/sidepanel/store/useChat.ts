import { create } from "zustand";

export type MessageRole = "user" | "assistant";

export interface ToolCard {
  tool_use_id: string;
  tool_name: string;
  input: unknown;
  output?: string;
  is_error?: boolean;
  done: boolean;
}

export interface ChatMessage {
  id: string;
  role: MessageRole;
  text: string;
  streaming: boolean;
  tools: ToolCard[];
  timestamp: number;
}

interface ChatStore {
  messages: ChatMessage[];
  isLoading: boolean;
  activeMessageId: string | null;
  addUserMessage: (content: string, id: string) => void;
  startAssistantMessage: (message_id: string) => void;
  appendTextDelta: (message_id: string, delta: string) => void;
  addToolStart: (message_id: string, tool: Omit<ToolCard, "done">) => void;
  resolveToolResult: (message_id: string, tool_use_id: string, output: string, is_error: boolean) => void;
  completeMessage: (message_id: string) => void;
  clearHistory: () => void;
}

export const useChat = create<ChatStore>((set, get) => ({
  messages: [],
  isLoading: false,
  activeMessageId: null,

  addUserMessage: (content, id) => {
    set((s) => ({
      messages: [
        ...s.messages,
        {
          id,
          role: "user",
          text: content,
          streaming: false,
          tools: [],
          timestamp: Date.now(),
        },
      ],
      isLoading: true,
    }));
  },

  startAssistantMessage: (message_id) => {
    set((s) => ({
      activeMessageId: message_id,
      messages: [
        ...s.messages,
        {
          id: message_id,
          role: "assistant",
          text: "",
          streaming: true,
          tools: [],
          timestamp: Date.now(),
        },
      ],
    }));
  },

  appendTextDelta: (message_id, delta) => {
    set((s) => ({
      messages: s.messages.map((m) =>
        m.id === message_id ? { ...m, text: m.text + delta } : m
      ),
    }));
  },

  addToolStart: (message_id, tool) => {
    set((s) => ({
      messages: s.messages.map((m) =>
        m.id === message_id
          ? { ...m, tools: [...m.tools, { ...tool, done: false }] }
          : m
      ),
    }));
  },

  resolveToolResult: (message_id, tool_use_id, output, is_error) => {
    set((s) => ({
      messages: s.messages.map((m) =>
        m.id === message_id
          ? {
              ...m,
              tools: m.tools.map((t) =>
                t.tool_use_id === tool_use_id
                  ? { ...t, output, is_error, done: true }
                  : t
              ),
            }
          : m
      ),
    }));
  },

  completeMessage: (message_id) => {
    set((s) => ({
      isLoading: false,
      activeMessageId: null,
      messages: s.messages.map((m) =>
        m.id === message_id ? { ...m, streaming: false } : m
      ),
    }));
  },

  clearHistory: () => set({ messages: [], isLoading: false, activeMessageId: null }),
}));
