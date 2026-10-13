"use strict";
/* Talks to the Python engine. Tauri uses the OS keychain command; the loopback preview keeps
   keys in the engine process only, never in a file. */
const bridge = (() => {
  const listeners = new Set();
  const progressListeners = new Set();
  let seq = 0;
  const pending = new Map();

  function isTauri() {
    return Boolean(window.__TAURI__ && window.__TAURI__.core);
  }

  function dispatch(message) {
    if (message.event === "progress") {
      for (const fn of progressListeners) fn(message.result || {});
      return;
    }
    const waiter = pending.get(String(message.id));
    if (!waiter) return;
    pending.delete(String(message.id));
    if (message.ok) waiter.resolve(message.result);
    else waiter.reject(new Error(message.error || "The engine reported a problem."));
  }

  async function init() {
    if (!isTauri()) return;
    await window.__TAURI__.event.listen("engine://line", (event) => {
      try { dispatch(JSON.parse(event.payload)); }
      catch { /* Ignore a torn line; the next message still arrives. */ }
    });
  }

  function call(method, params = {}) {
    const id = String(++seq);
    const result = new Promise((resolve, reject) => pending.set(id, {resolve, reject}));
    const body = JSON.stringify({id, method, params});
    if (isTauri()) {
      window.__TAURI__.core.invoke("engine_call", {request: body}).catch((error) => {
        pending.delete(id);
        reject(error instanceof Error ? error : new Error(String(error)));
      });
      return result;
    }
    fetch("/rpc", {method: "POST", headers: {"content-type": "application/json"}, body})
      .then(async (response) => {
        const reader = response.body.getReader();
        const decoder = new TextDecoder();
        let buffer = "";
        while (true) {
          const chunk = await reader.read();
          if (chunk.done) break;
          buffer += decoder.decode(chunk.value, {stream: true});
          let newline = buffer.indexOf("\n");
          while (newline >= 0) {
            const line = buffer.slice(0, newline);
            buffer = buffer.slice(newline + 1);
            if (line.trim()) dispatch(JSON.parse(line));
            newline = buffer.indexOf("\n");
          }
        }
      })
      .catch((error) => {
        if (pending.has(id)) {
          pending.delete(id);
          reject(error instanceof Error ? error : new Error(String(error)));
        }
      });
    return result;
  }

  async function saveKey(provider, secret) {
    if (isTauri()) {
      // "keychain" when the OS store persisted it, "memory" when there is no
      // working keychain and the key only lives in the engine for now.
      const where = await window.__TAURI__.core.invoke("keychain_set", {provider, secret});
      return where || "keychain";
    }
    const names = {
      openai: "OPENAI_API_KEY",
      anthropic: "ANTHROPIC_API_KEY",
      gemini: "GEMINI_API_KEY",
      xai: "XAI_API_KEY",
      openrouter: "OPENROUTER_API_KEY",
      compatible: "COMPATIBLE_API_KEY",
    };
    await call("set_keys", {keys: {[names[provider]]: secret}});
    return "memory";
  }

  async function saveText(filename, contents) {
    if (isTauri()) {
      return window.__TAURI__.core.invoke("save_text", {filename, contents});
    }
    const link = document.createElement("a");
    link.href = URL.createObjectURL(new Blob([contents], {type: "text/plain;charset=utf-8"}));
    link.download = filename;
    link.click();
    setTimeout(() => URL.revokeObjectURL(link.href), 1000);
    return filename;
  }

  async function openExternal(url) {
    if (isTauri() && window.__TAURI__.opener) {
      await window.__TAURI__.opener.openUrl(url);
      return;
    }
    window.open(url, "_blank", "noopener");
  }

  return {
    init, call, isTauri, saveKey, saveText, openExternal, dispatch,
    onProgress(fn) { progressListeners.add(fn); },
  };
})();
