// extension/firefox/background.js
// Chrome MV3 Service Worker: acts as the local bridge endpoint between ReflexDesk desktop runtime and tabs.

const DEFAULT_PORT = 8788;
const BRIDGE_WS_URL = `ws://127.0.0.1:${DEFAULT_PORT}/browser-bridge`;

let socket = null;
let reconnectTimer = null;
let pairingSecret = null;
const seenNonces = new Set();

// Load persisted pairing secret from chrome storage
chrome.storage.local.get(["pairingSecret"], (res) => {
  if (res.pairingSecret) {
    pairingSecret = res.pairingSecret;
  }
  connectToDesktop();
});

function connectToDesktop() {
  if (socket && (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING)) {
    return;
  }

  try {
    socket = new WebSocket(BRIDGE_WS_URL);

    socket.onopen = () => {
      console.log("[ReflexDesk Extension] Connected to desktop bridge");
      if (reconnectTimer) {
        clearInterval(reconnectTimer);
        reconnectTimer = null;
      }
      // Send handshake
      socket.send(JSON.stringify({
        type: "handshake",
        v: 1,
        pairing: pairingSecret || "",
        client: "firefox-mv3"
      }));
    };

    socket.onmessage = async (event) => {
      try {
        const msg = JSON.parse(event.data);
        const response = await handleDesktopMessage(msg);
        response.v = 1;
        response.pairing = pairingSecret || "";
        socket.send(JSON.stringify(response));
      } catch (err) {
        console.error("[ReflexDesk Extension] Error handling message:", err);
      }
    };

    socket.onclose = () => {
      socket = null;
      scheduleReconnect();
    };

    socket.onerror = (err) => {
      console.warn("[ReflexDesk Extension] Bridge websocket error:", err);
      if (socket) socket.close();
    };
  } catch (e) {
    scheduleReconnect();
  }
}

function scheduleReconnect() {
  if (!reconnectTimer) {
    reconnectTimer = setInterval(() => {
      connectToDesktop();
    }, 3000);
  }
}

async function getTargetTab(tabId) {
  if (tabId != null) {
    try {
      return await chrome.tabs.get(tabId);
    } catch (e) {
      return null;
    }
  }
  const [activeTab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
  return activeTab || null;
}

async function handleDesktopMessage(envelope) {
  if (envelope.v !== 1 || !pairingSecret || envelope.pairing !== pairingSecret) {
    return {
      v: 1,
      session: envelope.session || null,
      nonce: envelope.nonce || null,
      pairing: pairingSecret || "",
      success: false,
      error: "authentication-failed"
    };
  }
  if (typeof envelope.nonce !== "string" || envelope.nonce.length < 16 || seenNonces.has(envelope.nonce)) {
    return {
      v: 1,
      session: envelope.session || null,
      nonce: envelope.nonce || null,
      pairing: pairingSecret || "",
      success: false,
      error: "nonce-invalid-or-replayed"
    };
  }
  if (seenNonces.size >= 4096) seenNonces.clear();
  seenNonces.add(envelope.nonce);
  const { action, args, session, nonce, tabId } = envelope;

  // Handle browser lifecycle actions
  if (action === "browser.tabs") {
    const tabs = await chrome.tabs.query(args && args.currentWindowOnly ? { currentWindow: true } : {});
    return {
      session,
      nonce,
      success: true,
      data: {
        tabs: tabs.map(t => ({
          id: t.id,
          title: t.title || "",
          url: t.url || "",
          active: t.active,
          status: t.status,
          incognito: t.incognito
        }))
      }
    };
  }

  if (action === "browser.open") {
    const url = args.url;
    if (args.newTab) {
      const newTab = await chrome.tabs.create({ url, active: args.active !== false });
      return {
        session,
        nonce,
        success: true,
        data: { tabId: newTab.id, url: newTab.url, title: newTab.title }
      };
    } else {
      const tab = await getTargetTab(tabId);
      if (tab) {
        const updated = await chrome.tabs.update(tab.id, { url, active: true });
        return {
          session,
          nonce,
          success: true,
          data: { tabId: updated.id, url: updated.url, title: updated.title }
        };
      } else {
        const created = await chrome.tabs.create({ url, active: true });
        return {
          session,
          nonce,
          success: true,
          data: { tabId: created.id, url: created.url, title: created.title }
        };
      }
    }
  }

  if (action === "browser.download") {
    try {
      const downloadId = await chrome.downloads.download({
        url: args.url,
        filename: args.filename || undefined
      });
      return {
        session,
        nonce,
        success: true,
        data: { downloadId, state: "in_progress", filename: args.filename || null }
      };
    } catch (e) {
      return {
        session,
        nonce,
        success: false,
        error: "download-failed",
        message: e.message
      };
    }
  }

  // Forward DOM inspection and interaction actions to content script in target tab
  const targetTab = await getTargetTab(tabId);
  if (!targetTab) {
    return {
      session,
      nonce,
      success: false,
      error: "tab-not-found",
      message: `Target tab ${tabId} could not be resolved or focused`
    };
  }

  // Ensure content script injected if not already running
  try {
    const tabResult = await chrome.tabs.sendMessage(targetTab.id, { action, args });
    if (action === "browser.inspect" && tabResult.data) {
      tabResult.data.tabId = targetTab.id;
    }
    return {
      session,
      nonce,
      success: tabResult.success !== false,
      data: tabResult.data || tabResult,
      error: tabResult.error || null
    };
  } catch (err) {
    // Try re-injecting content script and retrying once
    try {
      await chrome.scripting.executeScript({
        target: { tabId: targetTab.id },
        files: ["content.js"]
      });
      const retryResult = await chrome.tabs.sendMessage(targetTab.id, { action, args });
      if (action === "browser.inspect" && retryResult.data) {
        retryResult.data.tabId = targetTab.id;
      }
      return {
        session,
        nonce,
        success: retryResult.success !== false,
        data: retryResult.data || retryResult,
        error: retryResult.error || null
      };
    } catch (injectErr) {
      return {
        session,
        nonce,
        success: false,
        error: "content-script-unavailable",
        message: `Failed communicating with content script: ${injectErr.message}`
      };
    }
  }
}

// Listen for options / popup updates to pairingSecret
chrome.runtime.onMessage.addListener((req, sender, sendResponse) => {
  if (req.type === "setPairingSecret") {
    pairingSecret = req.secret;
    chrome.storage.local.set({ pairingSecret }, () => {
      connectToDesktop();
      sendResponse({ success: true });
    });
    return true;
  }
});
