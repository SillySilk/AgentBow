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
    else if (cmd === "tab_list") {
      const allTabs = await chrome.tabs.query({});
      result = {
        tabs: allTabs.map(t => ({
          id: t.id, title: t.title, url: t.url,
          active: t.active, windowId: t.windowId, index: t.index,
          pinned: t.pinned, audible: t.audible, muted: t.mutedInfo?.muted,
        })),
      };
    }
    else if (cmd === "tab_new") {
      const newTab = await chrome.tabs.create({
        url: msg.url || "about:blank",
        active: msg.active !== false,
      });
      if (msg.url && msg.url !== "about:blank") {
        // Wait for load
        await new Promise<void>((resolve) => {
          const listener = (updatedTabId: number, changeInfo: chrome.tabs.TabChangeInfo) => {
            if (updatedTabId === newTab.id && changeInfo.status === "complete") {
              chrome.tabs.onUpdated.removeListener(listener);
              resolve();
            }
          };
          chrome.tabs.onUpdated.addListener(listener);
          setTimeout(() => { chrome.tabs.onUpdated.removeListener(listener); resolve(); }, 10000);
        });
      }
      const created = await chrome.tabs.get(newTab.id!);
      result = { id: created.id, url: created.url, title: created.title };
    }
    else if (cmd === "tab_close") {
      const tabIds: number[] = Array.isArray(msg.tab_ids) ? msg.tab_ids : [msg.tab_id];
      await chrome.tabs.remove(tabIds);
      result = { closed: tabIds };
    }
    else if (cmd === "tab_switch") {
      await chrome.tabs.update(msg.tab_id, { active: true });
      if (msg.window_id !== undefined) {
        await chrome.windows.update(msg.window_id, { focused: true });
      }
      const switched = await chrome.tabs.get(msg.tab_id);
      result = { id: switched.id, url: switched.url, title: switched.title };
    }
    else if (cmd === "back") {
      const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tabs[0]?.id) throw new Error("No active tab");
      await chrome.tabs.goBack(tabs[0].id);
      // Wait for load
      await new Promise(r => setTimeout(r, 500));
      const tab = await chrome.tabs.get(tabs[0].id);
      result = { url: tab.url, title: tab.title };
    }
    else if (cmd === "forward") {
      const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tabs[0]?.id) throw new Error("No active tab");
      await chrome.tabs.goForward(tabs[0].id);
      await new Promise(r => setTimeout(r, 500));
      const tab = await chrome.tabs.get(tabs[0].id);
      result = { url: tab.url, title: tab.title };
    }
    else if (cmd === "reload") {
      const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tabs[0]?.id) throw new Error("No active tab");
      const tabId = tabs[0].id;
      await chrome.tabs.reload(tabId, { bypassCache: !!msg.bypass_cache });
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
      const tab = await chrome.tabs.get(tabId);
      result = { url: tab.url, title: tab.title };
    }
    else if (cmd === "get_cookies") {
      const cookies = await chrome.cookies.getAll({ url: msg.url });
      result = {
        cookies: cookies.map(c => ({
          name: c.name, value: c.value, domain: c.domain, path: c.path,
          secure: c.secure, httpOnly: c.httpOnly,
          expirationDate: c.expirationDate, sameSite: c.sameSite,
        })),
      };
    }
    else if (cmd === "set_cookie") {
      const cookie = await chrome.cookies.set({
        url: msg.url,
        name: msg.name,
        value: msg.value,
        domain: msg.domain,
        path: msg.path || "/",
        secure: msg.secure,
        httpOnly: msg.httpOnly,
        sameSite: msg.sameSite || "lax",
        expirationDate: msg.expirationDate,
      });
      result = { cookie };
    }
    else if (cmd === "delete_cookies") {
      const cookies = await chrome.cookies.getAll({ url: msg.url });
      const nameFilter = msg.name;
      const toDelete = nameFilter ? cookies.filter(c => c.name === nameFilter) : cookies;
      for (const c of toDelete) {
        const protocol = c.secure ? "https" : "http";
        await chrome.cookies.remove({ url: `${protocol}://${c.domain}${c.path}`, name: c.name });
      }
      result = { deleted: toDelete.length };
    }
    else if (cmd === "read_page") {
      const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tabs[0]?.id) throw new Error("No active tab");
      const mode = msg.mode || "text"; // "text", "html", or "links"
      const results2 = await chrome.scripting.executeScript({
        target: { tabId: tabs[0].id },
        func: (m: string) => {
          if (m === "html") return document.documentElement.outerHTML;
          if (m === "links") {
            return JSON.stringify(
              Array.from(document.querySelectorAll("a[href]")).map(a => ({
                text: (a as HTMLAnchorElement).innerText.trim().slice(0, 100),
                href: (a as HTMLAnchorElement).href,
              })).filter(l => l.text && l.href)
            );
          }
          // "text" mode — semantic content
          const clone = document.body.cloneNode(true) as HTMLElement;
          clone.querySelectorAll("script,style,nav,footer,header,aside,iframe,noscript,svg").forEach(el => el.remove());
          return clone.innerText.replace(/\n{3,}/g, "\n\n").trim().slice(0, 50000);
        },
        args: [mode],
      });
      result = { content: results2?.[0]?.result ?? "", url: tabs[0].url, title: tabs[0].title };
    }
    else if (cmd === "click") {
      const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tabs[0]?.id) throw new Error("No active tab");
      const results2 = await chrome.scripting.executeScript({
        target: { tabId: tabs[0].id },
        func: (selector: string) => {
          const el = document.querySelector(selector) as HTMLElement | null;
          if (!el) return "ERROR: Element not found: " + selector;
          el.click();
          return "Clicked: " + selector;
        },
        args: [msg.selector],
      });
      result = { result: results2?.[0]?.result ?? "no result" };
    }
    else if (cmd === "fill") {
      const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tabs[0]?.id) throw new Error("No active tab");
      const results2 = await chrome.scripting.executeScript({
        target: { tabId: tabs[0].id },
        func: (selector: string, value: string, submit: boolean) => {
          const el = document.querySelector(selector) as HTMLInputElement | HTMLTextAreaElement | null;
          if (!el) return "ERROR: Element not found: " + selector;
          el.focus();
          el.value = value;
          el.dispatchEvent(new Event("input", { bubbles: true }));
          el.dispatchEvent(new Event("change", { bubbles: true }));
          if (submit) {
            const form = el.closest("form");
            if (form) form.submit();
            else return "Filled but no form found to submit";
          }
          return "Filled: " + selector;
        },
        args: [msg.selector, msg.value, !!msg.submit],
      });
      result = { result: results2?.[0]?.result ?? "no result" };
    }
    else if (cmd === "get_url") {
      const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
      if (!tabs[0]) throw new Error("No active tab");
      result = { url: tabs[0].url, title: tabs[0].title, id: tabs[0].id };
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
