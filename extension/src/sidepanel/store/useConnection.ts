import { create } from "zustand";
import { v4 as uuidv4 } from "uuid";

export type ConnectionStatus = "disconnected" | "connecting" | "connected" | "error";

interface PageContext {
  url: string;
  title: string;
}

interface ConnectionStore {
  status: ConnectionStatus;
  statusMessage: string;
  currentPage: PageContext | null;
  setStatus: (status: ConnectionStatus, message?: string) => void;
  setCurrentPage: (page: PageContext) => void;
  sendUserMessage: (content: string) => Promise<string | null>;
  sendInterrupt: () => void;
}

export const useConnection = create<ConnectionStore>((set, get) => ({
  status: "disconnected",
  statusMessage: "",
  currentPage: null,

  setStatus: (status, message = "") => set({ status, statusMessage: message }),
  setCurrentPage: (page) => set({ currentPage: page }),

  sendUserMessage: async (content) => {
    const message_id = uuidv4();
    return new Promise((resolve) => {
      chrome.runtime.sendMessage(
        { type: "send_user_message", content, message_id },
        (response) => {
          if (response?.ok) {
            resolve(message_id);
          } else {
            resolve(null);
          }
        }
      );
    });
  },

  sendInterrupt: () => {
    chrome.runtime.sendMessage({ type: "send_interrupt" });
  },
}));
