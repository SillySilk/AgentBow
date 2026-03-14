const WS_PORT = 9357;
const KEEPALIVE_ALARM = "bow-keepalive";
const RECONNECT_DELAYS = [1000, 2000, 4000, 8000, 16000, 30000];

let ws: WebSocket | null = null;
let reconnectAttempt = 0;
let authenticated = false;
let sessionId = generateSessionId();

// ── Keepalive: multiple strategies to prevent MV3 suspension ──────────────────

// Strategy 1: Alarm every 25s
chrome.alarms.create(KEEPALIVE_ALARM, { periodInMinutes: 0.4 });
chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === KEEPALIVE_ALARM) {
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ type: "ping" }));
    } else if (!ws || ws.readyState === WebSocket.CLOSED) {
      connect();
    }
  }
});

// Strategy 2: Self-messaging keepalive — keeps service worker alive during active tasks
let keepAliveInterval: ReturnType<typeof setInterval> | null = null;

function startKeepAlive() {
  if (keepAliveInterval) return;
  keepAliveInterval = setInterval(() => {
    // Accessing chrome.runtime keeps the SW alive
    chrome.runtime.getPlatformInfo(() => {});
  }, 20000);
}

function stopKeepAlive() {
  if (keepAliveInterval) {
    clearInterval(keepAliveInterval);
    keepAliveInterval = null;
  }
}

// Strategy 3: Port-based keepalive from sidepanel
chrome.runtime.onConnect.addListener((port) => {
  if (port.name === "bow-keepalive") {
    // Port being open keeps the service worker alive
    port.onDisconnect.addListener(() => {
      // Sidepanel closed — but tasks may still be running
    });
  }
});

// ── Connect ───────────────────────────────────────────────────────────────────

function connect() {
  if (ws && ws.readyState !== WebSocket.CLOSED) return;

  broadcastStatus("connecting");
  ws = new WebSocket(`ws://127.0.0.1:${WS_PORT}`);

  ws.onopen = async () => {
    reconnectAttempt = 0;
    const token = await getToken();
    if (!token) {
      broadcastStatus("error", "No token set — open Settings tab");
      return;
    }
    ws!.send(JSON.stringify({ type: "auth", token, session_id: sessionId }));
  };

  ws.onmessage = (event) => {
    try {
      const msg = JSON.parse(event.data as string);
      if (msg.type === "auth_ok") {
        authenticated = true;
        broadcastStatus("connected");
        startKeepAlive();
        sendCurrentPageContext();
      } else if (msg.type === "auth_error") {
        authenticated = false;
        broadcastStatus("error", msg.message || "Auth failed");
        ws?.close();
      } else if (msg.type === "browser_cmd") {
        handleBrowserCmd(msg);
      } else {
        // Forward all other messages to sidepanel
        chrome.runtime.sendMessage(msg).catch(() => {});
      }
    } catch (e) {
      console.error("[Bow] Failed to parse WS message", e);
    }
  };

  ws.onerror = () => {
    broadcastStatus("error", "Connection error");
  };

  ws.onclose = () => {
    authenticated = false;
    stopKeepAlive();
    broadcastStatus("disconnected");
    scheduleReconnect();
  };
}

async function handleBrowserCmd(msg: any) {
  const { request_id, cmd } = msg;
  try {
    let result: any = {};

    if (cmd === "screenshot") {
      const dataUrl = await chrome.tabs.captureVisibleTab({ format: "png" });
      result = { data: dataUrl };
    }
    else if (cmd === "exec_js") {
      const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tabs[0]?.id) throw new Error("No active tab");
      const results = await chrome.scripting.executeScript({
        target: { tabId: tabs[0].id },
        func: (code: string) => {
          try { return String(eval(code)); } catch(e: any) { return "ERROR: " + e.message; }
        },
        args: [msg.js],
      });
      result = { result: results?.[0]?.result ?? "(no result)" };
    }
    else if (cmd === "navigate") {
      const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tabs[0]?.id) throw new Error("No active tab");
      const tabId = tabs[0].id;
      await chrome.tabs.update(tabId, { url: msg.url });
      // Wait for navigation to complete (max 10s)
      await new Promise<void>((resolve) => {
        const listener = (updatedTabId: number, changeInfo: chrome.tabs.TabChangeInfo) => {
          if (updatedTabId === tabId && changeInfo.status === "complete") {
            chrome.tabs.onUpdated.removeListener(listener);
            resolve();
          }
        };
        chrome.tabs.onUpdated.addListener(listener);
        setTimeout(() => { chrome.tabs.onUpdated.removeListener(listener); resolve(); }, 10000);
      });
      const updatedTab = await chrome.tabs.get(tabId);
      result = { url: updatedTab.url };
    }

    ws?.send(JSON.stringify({ type: "browser_result", request_id, ...result }));
  } catch (e: any) {
    ws?.send(JSON.stringify({ type: "browser_result", request_id, error: e.message }));
  }
}

function scheduleReconnect() {
  const delay = RECONNECT_DELAYS[Math.min(reconnectAttempt, RECONNECT_DELAYS.length - 1)];
  reconnectAttempt++;
  setTimeout(connect, delay);
}

// ── Tab events ────────────────────────────────────────────────────────────────

chrome.tabs.onActivated.addListener(({ tabId }) => {
  extractAndSendPageContext(tabId);
});

chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (changeInfo.status === "complete") {
    chrome.tabs.query({ active: true, currentWindow: true }, (tabs) => {
      if (tabs[0]?.id === tabId) {
        extractAndSendPageContext(tabId);
      }
    });
  }
});

async function sendCurrentPageContext() {
  const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
  if (tabs[0]?.id) {
    extractAndSendPageContext(tabs[0].id);
  }
}

async function extractAndSendPageContext(tabId: number) {
  if (!ws || ws.readyState !== WebSocket.OPEN || !authenticated) return;

  try {
    const results = await chrome.scripting.executeScript({
      target: { tabId },
      files: ["content/page-extractor.js"],
    });

    const ctx = results?.[0]?.result as {
      url: string;
      title: string;
      selectedText: string;
      pageText: string;
    } | null;

    if (ctx) {
      ws.send(
        JSON.stringify({
          type: "page_context",
          url: ctx.url,
          title: ctx.title,
          selected_text: ctx.selectedText,
          page_text: ctx.pageText,
        })
      );
      // Also send to sidepanel so it can update UI
      chrome.runtime.sendMessage({
        type: "_page_context_update",
        url: ctx.url,
        title: ctx.title,
      }).catch(() => {});
    }
  } catch (e) {
    // Scripting may fail on chrome:// pages — ignore
  }
}

// ── Messages from sidepanel ───────────────────────────────────────────────────

chrome.runtime.onMessage.addListener((msg, _sender, sendResponse) => {
  if (msg.type === "send_user_message") {
    if (!ws || ws.readyState !== WebSocket.OPEN || !authenticated) {
      sendResponse({ error: "Not connected" });
      return true;
    }
    ws.send(JSON.stringify({
      type: "user_message",
      content: msg.content,
      message_id: msg.message_id,
    }));
    sendResponse({ ok: true });
    return true;
  }

  if (msg.type === "send_interrupt") {
    ws?.send(JSON.stringify({ type: "interrupt", session_id: sessionId }));
    sendResponse({ ok: true });
    return true;
  }

  if (msg.type === "get_status") {
    sendResponse({
      status: authenticated ? "connected"
        : ws?.readyState === WebSocket.CONNECTING ? "connecting"
        : "disconnected",
    });
    return true;
  }

  if (msg.type === "set_token") {
    chrome.storage.local.set({ bow_token: msg.token }, () => {
      // Force-kill old connection and reconnect with new token
      if (ws) {
        ws.onclose = null; // prevent scheduleReconnect from old socket
        ws.close();
        ws = null;
      }
      authenticated = false;
      reconnectAttempt = 0;
      sessionId = generateSessionId();
      connect();
    });
    sendResponse({ ok: true });
    return true;
  }
});

// ── Action click → open side panel ───────────────────────────────────────────

chrome.action.onClicked.addListener((tab) => {
  chrome.sidePanel.open({ tabId: tab.id! });
});

// ── Helpers ───────────────────────────────────────────────────────────────────

async function getToken(): Promise<string | null> {
  return new Promise((resolve) => {
    chrome.storage.local.get("bow_token", (result) => {
      resolve(result.bow_token || null);
    });
  });
}

function broadcastStatus(status: string, message?: string) {
  chrome.runtime.sendMessage({
    type: "_connection_status",
    status,
    message,
  }).catch(() => {});
}

function generateSessionId(): string {
  return Math.random().toString(36).slice(2) + Date.now().toString(36);
}

// ── Init ──────────────────────────────────────────────────────────────────────
connect();
