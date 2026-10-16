"use strict";
/* Side by side: one question to up to four AIs through OpenRouter. Each AI
   streams into its own pane with every number worth comparing: when it
   connected, its first token, thinking and writing time, speed, tokens,
   how much of its context it used, cost, and who served it. The engine
   makes the requests; this file only draws. */
const sideBySide = (() => {
  const MAX = 4;
  // A typical question and reply, for "about ₹x a reply" in the picker.
  const TYPICAL = {input: 200, output: 600};
  const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  // Symbols over words: each idea has one icon, and the words live in tooltips.
  const ICON = {
    start: '<path d="M8 5.5v13l11-6.5z"/>',
    speed: '<path d="M13 3L5 14h6l-1 7 8-11h-6z"/>',
    time: '<circle cx="12" cy="13" r="8"/><path d="M12 9v4l2.5 2M10 2h4"/>',
    flag: '<path d="M5 21V4M5 4h11l-2.5 4L16 12H5"/>',
    done: '<circle cx="12" cy="12" r="9"/><path d="M8 12.5l3 3 5-6"/>',
    error: '<path d="M12 3.5l9 16H3z"/><path d="M12 10v4M12 17h.01"/>',
    stopped: '<rect x="6.5" y="6.5" width="11" height="11" rx="2"/>',
    thinking: '<path d="M7.5 16.5a4 4 0 0 1-.4-7.97A5 5 0 0 1 16.6 8a3.5 3.5 0 1 1 .4 7H7.5z"/><circle cx="6" cy="20" r="1"/>',
    details: '<path d="M5 20V11M12 20V4M19 20v-6"/>',
    copy: '<rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V6a2 2 0 0 0-2-2H6a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h2"/>',
    total: '<ellipse cx="12" cy="6.5" rx="7" ry="2.8"/><path d="M5 6.5v5.5c0 1.5 3.1 2.8 7 2.8s7-1.3 7-2.8V6.5M5 12v5.5c0 1.5 3.1 2.8 7 2.8s7-1.3 7-2.8V12"/>',
  };
  const icon = (name, cls = "icon") => svg(ICON[name], cls);

  const S = {
    status: null,
    models: [],
    loaded: false,
    picks: [],
    query: "",
    onlyFree: false,
    ask: null,
    view: "answer",
    ticking: false,
  };

  /* ---------- small helpers ---------- */

  const num = new Intl.NumberFormat("en-IN");
  const tokens = (n) => (n === null || n === undefined ? "—" : num.format(n));
  function secs(s) {
    if (s === null || s === undefined || Number.isNaN(s)) return "—";
    if (s < 1) return s.toFixed(2) + " s";
    if (s < 10) return s.toFixed(1) + " s";
    if (s < 60) return Math.round(s) + " s";
    return `${Math.floor(s / 60)} min ${Math.round(s % 60)} s`;
  }
  const ms = (v) => (v === null || v === undefined ? "—" : secs(v / 1000));
  const money = (usd) => (usd === null || usd === undefined ? "—" : usd === 0 ? t("ai.free") : rupees(usd));
  function contextLabel(n) {
    if (!n) return "—";
    if (n >= 1e6) return String(Math.round(n / 1e5) / 10) + "M";
    if (n >= 1000) return Math.round(n / 1000) + "K";
    return String(n);
  }
  function model(id) {
    return S.models.find((m) => m.id === id) || {id, name: id};
  }
  function splitName(m) {
    const name = m.name || m.id;
    const at = name.indexOf(": ");
    return at > 0 ? {maker: name.slice(0, at), short: name.slice(at + 2)} : {maker: m.id.split("/")[0], short: name};
  }
  function typicalCost(m) {
    if (m.free) return t("ai.free");
    if (m.input_usd === null || m.input_usd === undefined) return t("side.priceUnknown");
    return t("side.perReply", {money: rupees(m.input_usd * TYPICAL.input + (m.output_usd || 0) * TYPICAL.output)});
  }
  function setStatus(text) {
    $("side-status").textContent = text || "";
  }
  const connected = () => Boolean(S.status && S.status.connected && S.status.ok !== false);

  /* ---------- motion ---------- */

  // Analytic damped spring, the same family as the launch film.
  function spring({from = 0, to = 1, delay = 0, k = 170, c = 26, onUpdate, onDone}) {
    if (reduceMotion) {
      onUpdate(to);
      if (onDone) onDone();
      return;
    }
    const w0 = Math.sqrt(k), z = c / (2 * Math.sqrt(k));
    const end = Math.log(2000) / (Math.min(z, 1) * w0);
    const at = (time) => {
      if (time <= 0) return 0;
      let x;
      if (z < 1) {
        const wd = w0 * Math.sqrt(1 - z * z);
        x = Math.exp(-z * w0 * time) * (Math.cos(wd * time) + (z * w0 / wd) * Math.sin(wd * time));
      } else {
        x = Math.exp(-w0 * time) * (1 + w0 * time);
      }
      return 1 - x;
    };
    const start = performance.now() + delay;
    const frame = (now) => {
      const time = (now - start) / 1000;
      if (time >= end) {
        onUpdate(to);
        if (onDone) onDone();
        return;
      }
      onUpdate(from + (to - from) * at(time));
      requestAnimationFrame(frame);
    };
    requestAnimationFrame(frame);
  }

  // Grow each element from one rectangle into the place the layout gives it.
  function growFrom(element, from, to, container, options = {}) {
    const box = container.getBoundingClientRect();
    const a = {x: from.left - box.left, y: from.top - box.top, w: from.width, h: from.height};
    const b = {x: to.left - box.left, y: to.top - box.top, w: to.width, h: to.height};
    Object.assign(element.style, {position: "absolute", margin: "0", left: a.x + "px", top: a.y + "px", width: a.w + "px", height: a.h + "px"});
    spring({
      ...options,
      onUpdate: (p) => {
        element.style.left = a.x + (b.x - a.x) * p + "px";
        element.style.top = a.y + (b.y - a.y) * p + "px";
        element.style.width = Math.max(40, a.w + (b.w - a.w) * p) + "px";
        element.style.height = Math.max(40, a.h + (b.h - a.h) * p) + "px";
      },
      onDone: () => {
        for (const name of ["position", "margin", "left", "top", "width", "height"]) element.style[name] = "";
        if (options.onDone) options.onDone();
      },
    });
  }

  // The number on the Ask button rolls when it changes.
  function roll(node, value) {
    if (node.textContent === String(value)) return;
    node.textContent = String(value);
    if (!reduceMotion && node.animate) {
      node.animate([{transform: "translateY(60%)", opacity: 0}, {transform: "translateY(0)", opacity: 1}],
        {duration: 260, easing: "cubic-bezier(.2,.9,.3,1.2)"});
    }
  }

  function pop(node, delay = 0) {
    if (reduceMotion || !node.animate) return;
    node.animate([{transform: "scale(.6)", opacity: 0}, {transform: "scale(1.06)", opacity: 1, offset: 0.7}, {transform: "scale(1)", opacity: 1}],
      {duration: 380, delay, easing: "ease-out", fill: "backwards"});
  }

  /* ---------- key, models and picks ---------- */

  async function loadStatus() {
    setStatus(t("side.checking"));
    try {
      S.status = await bridge.call("live_status");
    } catch (error) {
      S.status = {connected: false, error: error.message};
    }
    renderKey();
  }

  function renderKey() {
    const status = S.status || {};
    $("side-key").hidden = connected();
    $("side-key-error").textContent = status.connected && status.ok === false ? status.error : (status.error || "");
    if (connected()) {
      const left = status.credit_left_usd ?? status.limit_remaining_usd;
      setStatus(left === null || left === undefined ? t("side.connected") : t("side.connectedCredit", {money: rupees(left)}));
    } else {
      setStatus(t("side.needKey"));
    }
    syncAsk();
  }

  async function saveKey() {
    const input = $("side-key-input");
    const key = input.value.trim();
    if (!key) {
      input.focus();
      return;
    }
    $("side-key-save").disabled = true;
    $("side-key-error").textContent = "";
    try {
      await bridge.saveKey("openrouter", key);
      input.value = "";
      await loadStatus();
      if (connected()) $("side-prompt").focus();
    } catch (error) {
      $("side-key-error").textContent = error.message;
    } finally {
      $("side-key-save").disabled = false;
    }
  }

  async function loadModels() {
    try {
      const reply = await bridge.call("live_models");
      S.models = reply.models || [];
    } catch (error) {
      S.models = [];
      setStatus(error.message);
    }
    restorePicks();
  }

  function restorePicks() {
    let saved = [];
    try {
      saved = JSON.parse(localStorage.getItem("km.side.picks") || "[]");
    } catch { /* Start fresh. */ }
    const known = new Set(S.models.map((m) => m.id));
    S.picks = saved.filter((id) => known.has(id)).slice(0, MAX);
    if (!S.picks.length) S.picks = suggestedPicks();
    renderPicks();
  }

  // Without saved picks: the newest flagship from four big makers.
  function suggestedPicks() {
    const picks = [];
    for (const prefix of ["anthropic/claude-sonnet", "openai/gpt", "google/gemini", "meta-llama/llama", "deepseek/", "qwen/"]) {
      const match = S.models
        .filter((m) => m.id.startsWith(prefix) && !m.free && !/[-.:](mini|nano|lite|flash|vision|image|audio|embed|guard|preview|search)\b/.test(m.id))
        .sort((a, b) => (b.created || 0) - (a.created || 0))[0];
      if (match) picks.push(match.id);
      if (picks.length === MAX) break;
    }
    return picks;
  }

  function savePicks() {
    try {
      localStorage.setItem("km.side.picks", JSON.stringify(S.picks));
    } catch { /* The picks still apply now. */ }
  }

  function renderPicks() {
    const box = $("side-picks");
    box.replaceChildren();
    for (const id of S.picks) {
      const name = splitName(model(id));
      box.append(el("span", {class: "pick-pill", title: id},
        el("span", {text: name.short}),
        el("button", {type: "button", class: "pill-x", "aria-label": t("side.remove", {ai: name.short}), text: "×",
          onclick: () => {
            S.picks = S.picks.filter((p) => p !== id);
            savePicks();
            renderPicks();
          }})));
    }
    box.append(el("button", {type: "button", class: "pick-add", onclick: openTray,
      text: S.picks.length >= MAX ? t("side.change") : S.picks.length ? t("side.addAnother") : t("side.addAi")}));
    syncAsk();
  }

  function syncAsk() {
    const n = S.picks.length;
    roll($("side-count"), n);
    $("side-ais").textContent = n === 1 ? t("side.ai") : t("side.ais");
    $("side-go").disabled = !n || !connected() || Boolean(S.ask && S.ask.running);
  }

  /* ---------- the picker tray ---------- */

  function openTray() {
    const tray = $("side-tray");
    tray.hidden = false;
    $("tray-search").value = S.query;
    renderTray();
    const sheet = tray.querySelector(".tray");
    spring({k: 210, c: 26, onUpdate: (p) => {
      sheet.style.transform = `translateY(${(1 - p) * 104}%)`;
      tray.style.backgroundColor = `rgba(0,0,0,${(0.42 * Math.min(1, p)).toFixed(3)})`;
    }});
    setTimeout(() => $("tray-search").focus(), 60);
  }

  function closeTray() {
    const tray = $("side-tray");
    if (tray.hidden) return;
    const sheet = tray.querySelector(".tray");
    spring({k: 260, c: 32, onUpdate: (p) => {
      sheet.style.transform = `translateY(${p * 104}%)`;
      tray.style.backgroundColor = `rgba(0,0,0,${(0.42 * (1 - Math.min(1, p))).toFixed(3)})`;
    }, onDone: () => { tray.hidden = true; }});
    renderPicks();
  }

  function renderTray() {
    const query = S.query.trim().toLowerCase();
    let list = S.models.filter((m) => !query || m.id.toLowerCase().includes(query) || (m.name || "").toLowerCase().includes(query));
    if (S.onlyFree) list = list.filter((m) => m.free);
    list.sort((a, b) => (S.picks.includes(b.id) - S.picks.includes(a.id)) || (b.created || 0) - (a.created || 0));
    const shown = list.slice(0, 150);
    $("tray-count").textContent = t("side.picked", {n: S.picks.length, max: MAX});
    const ul = $("tray-list");
    ul.replaceChildren();
    if (!S.models.length) {
      ul.append(el("li", {class: "muted", text: t("side.noModels")}));
      return;
    }
    for (const m of shown) {
      const picked = S.picks.includes(m.id);
      const name = splitName(m);
      const box = el("input", {type: "checkbox", checked: picked, disabled: !picked && S.picks.length >= MAX,
        onchange: () => {
          if (S.picks.includes(m.id)) S.picks = S.picks.filter((p) => p !== m.id);
          else if (S.picks.length < MAX) S.picks.push(m.id);
          savePicks();
          renderTray();
        }});
      ul.append(el("li", {},
        el("label", {class: "tray-item" + (picked ? " on" : "")}, box,
          el("span", {class: "tray-text"},
            el("span", {class: "tray-name", text: name.short}),
            el("span", {class: "tray-meta", text: [name.maker, t("side.contextShort", {n: contextLabel(m.context)}), typicalCost(m)].join(" · ")})))));
    }
    if (list.length > shown.length) ul.append(el("li", {class: "muted small", text: t("side.more", {n: list.length - shown.length})}));
  }

  /* ---------- asking ---------- */

  function newPane(id, slot) {
    const m = model(id);
    const name = splitName(m);
    const pane = {id, slot, m, text: "", thinking: "", connected: null, firstThinking: null, firstAnswer: null,
      done: false, metrics: null, server: null, error: null, dirty: true, view: S.view, follow: true};
    const state = el("span", {class: "pane-state busy", role: "img"}, el("i", {class: "pulse"}));
    // Four numbers, each behind a symbol: starts, speed, time, cost.
    const stat = (symbol, tip) => el("div", {class: "stat", title: t(tip)},
      symbol ? icon(symbol, "icon stat-icon") : el("span", {class: "stat-icon rupee", text: "₹"}),
      el("span", {class: "stat-value", text: "…"}));
    const stats = el("div", {class: "pane-stats"},
      stat("start", "side.tipStart"), stat("speed", "side.tipSpeed"), stat("time", "side.tipTime"), stat(null, "side.tipCost"));
    // A race bar on one shared time scale, with a flag at the finish.
    const flag = el("span", {class: "finish", hidden: true}, icon("flag"));
    const track = el("div", {class: "track", title: t("side.tipTrack")},
      el("span", {class: "tseg send"}), el("span", {class: "tseg wait"}), el("span", {class: "tseg think"}), el("span", {class: "tseg write"}), flag);
    const thinking = el("details", {class: "thinking", hidden: true},
      el("summary", {}, icon("thinking"), el("span", {text: t("side.thinking")})), el("div", {class: "thinking-text"}));
    const answer = el("div", {class: "answer"});
    const body = el("div", {class: "pane-body"}, thinking, answer);
    body.addEventListener("scroll", () => {
      pane.follow = body.scrollHeight - body.scrollTop - body.clientHeight < 40;
    });
    const metrics = el("div", {class: "pane-metrics", hidden: true});
    const toggle = el("button", {type: "button", class: "icon-btn", "aria-pressed": "false", title: t("side.metrics"), "aria-label": t("side.metrics"),
      onclick: () => setPaneView(pane, pane.view === "metrics" ? "answer" : "metrics")}, icon("details"));
    const copy = el("button", {type: "button", class: "icon-btn", title: t("side.copy"), "aria-label": t("side.copy"), onclick: async () => {
      try {
        await navigator.clipboard.writeText(pane.text);
        copy.classList.add("done");
        setTimeout(() => copy.classList.remove("done"), 1200);
      } catch { /* Selecting the text still works. */ }
    }}, icon("copy"));
    const node = el("article", {class: "pane", "data-slot": String(slot)},
      el("div", {class: "pane-inner"},
        el("header", {class: "pane-head"},
          el("div", {class: "pane-name"}, el("strong", {text: name.short, title: m.id}), el("span", {class: "pane-sub", text: name.maker})),
          el("span", {class: "pane-badges"}), state, toggle, copy),
        stats, track, body, metrics));
    Object.assign(pane, {node, state, stats, track, flag, thinkingBox: thinking, answer, body, metricsBox: metrics, toggle});
    setState(pane, "sending");
    return pane;
  }

  function setPaneView(pane, view) {
    pane.view = view;
    const metrics = view === "metrics";
    pane.body.hidden = metrics;
    pane.metricsBox.hidden = !metrics;
    pane.toggle.setAttribute("aria-pressed", String(metrics));
    if (metrics) renderMetrics(pane);
    const shown = metrics ? pane.metricsBox : pane.body;
    if (!reduceMotion && shown.animate) {
      shown.animate([{opacity: 0, transform: `translateX(${metrics ? 14 : -14}px)`}, {opacity: 1, transform: "none"}],
        {duration: 220, easing: "cubic-bezier(.2,.8,.2,1)"});
    }
  }

  function setView(view) {
    S.view = view;
    for (const button of document.querySelectorAll("#side-view button")) {
      button.setAttribute("aria-pressed", String(button.dataset.view === view));
    }
    if (S.ask) S.ask.panes.forEach((pane, index) => setTimeout(() => setPaneView(pane, view), index * 45));
  }

  async function ask(event) {
    event.preventDefault();
    const prompt = $("side-prompt").value.trim();
    if (!prompt) {
      setStatus(t("side.empty"));
      $("side-prompt").focus();
      return;
    }
    if (!S.picks.length) {
      openTray();
      return;
    }
    if (!connected() || (S.ask && S.ask.running)) return;
    const id = "ask-" + Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
    const from = $("side-composer").getBoundingClientRect();
    const askState = {id, prompt, running: true, started: performance.now(), panes: S.picks.map(newPane)};
    S.ask = askState;
    syncAsk();
    showRun(from);
    startTicking();
    try {
      const reply = await bridge.call("live_ask", {ask_id: id, prompt, models: askState.panes.map((p) => p.id)});
      for (const [index, result] of (reply.results || []).entries()) {
        const pane = askState.panes[index];
        if (pane && !pane.done) finishPane(pane, result);
        if (pane && result.server) {
          pane.server = result.server;
          if (pane.view === "metrics") renderMetrics(pane);
        }
      }
    } catch (error) {
      for (const pane of askState.panes) if (!pane.done) finishPane(pane, {error: error.message});
    }
    askState.running = false;
    if (S.ask === askState) {
      $("side-stop").hidden = true;
      syncAsk();
    }
  }

  function showRun(from) {
    const run = $("side-run");
    const stage = $("side-stage");
    $("side-intro").hidden = true;
    $("side-composer").hidden = true;
    $("side-key").hidden = true;
    $("side-status").hidden = true;
    run.hidden = false;
    $("side-bar-text").textContent = S.ask.prompt;
    $("side-stop").hidden = false;
    $("side-summary").hidden = true;
    stage.dataset.count = String(S.ask.panes.length);
    stage.replaceChildren(...S.ask.panes.map((p) => p.node));
    fitStage();
    for (const pane of S.ask.panes) setPaneView(pane, S.view);
    // The question card splits into one pane per AI.
    const bar = $("side-bar");
    if (!reduceMotion && bar.animate) {
      bar.animate([{opacity: 0, transform: `translateY(${Math.max(0, from.top - bar.getBoundingClientRect().top)}px) scale(.98)`}, {opacity: 1, transform: "none"}],
        {duration: 420, easing: "cubic-bezier(.2,.9,.25,1)"});
    }
    stage.classList.add("splitting");
    // Measure every cell before any pane leaves the grid, or the rest reflow.
    const targets = S.ask.panes.map((pane) => pane.node.getBoundingClientRect());
    S.ask.panes.forEach((pane, index) => {
      growFrom(pane.node, from, targets[index], stage, {delay: index * 55, k: 150, c: 21, onDone: () => {
        pane.node.classList.add("in");
        if (index === S.ask.panes.length - 1) stage.classList.remove("splitting");
      }});
    });
  }

  function fitStage() {
    const stage = $("side-stage");
    if ($("side-run").hidden) return;
    const top = stage.getBoundingClientRect().top + window.scrollY;
    stage.style.height = Math.max(360, window.innerHeight - top - 64) + "px";
  }

  function newQuestion() {
    if (S.ask && S.ask.running) stop();
    const run = $("side-run");
    const composer = $("side-composer");
    const panes = [...$("side-stage").children];
    const settle = () => {
      run.hidden = true;
      $("side-status").hidden = false;
      $("side-intro").hidden = false;
      composer.hidden = false;
      renderKey();
      const prompt = $("side-prompt");
      prompt.focus();
      prompt.select();
      if (!reduceMotion && composer.animate) {
        composer.animate([{opacity: 0, transform: "translateY(-18px) scale(.98)"}, {opacity: 1, transform: "none"}],
          {duration: 380, easing: "cubic-bezier(.2,.9,.25,1)"});
      }
    };
    if (reduceMotion || !panes.length || !panes[0].animate) {
      settle();
      return;
    }
    panes.forEach((node, index) => node.animate([{opacity: 1, transform: "none"}, {opacity: 0, transform: "translateY(-24px) scale(.94)"}],
      {duration: 240, delay: index * 30, easing: "cubic-bezier(.4,0,.6,1)", fill: "forwards"}));
    setTimeout(settle, 240 + panes.length * 30);
  }

  function stop() {
    if (!S.ask || !S.ask.running) return;
    $("side-stop").disabled = true;
    for (const pane of S.ask.panes) if (!pane.done) setState(pane, "stopping");
    bridge.call("live_stop", {ask_id: S.ask.id})
      .catch((error) => setStatus(error.message))
      .finally(() => { $("side-stop").disabled = false; });
  }

  /* ---------- streaming ---------- */

  function onEvent(event) {
    if (!event || event.kind !== "live" || !S.ask || event.ask_id !== S.ask.id) return;
    const pane = S.ask.panes[event.slot];
    if (!pane) return;
    if (event.type === "start") setState(pane, "sending");
    else if (event.type === "connected") {
      pane.connected = event.at;
      setState(pane, "waiting");
    } else if (event.type === "first") {
      if (event.thinking) {
        pane.firstThinking = event.at;
        pane.thinkingBox.hidden = false;
        pane.thinkingBox.open = true;
        setState(pane, "thinking");
      } else {
        pane.firstAnswer = event.at;
        pane.thinkingBox.open = false;
        setState(pane, "writing");
      }
    } else if (event.type === "text") {
      if (event.thinking) pane.thinking += event.text;
      else pane.text += event.text;
      pane.dirty = true;
    } else if (event.type === "done") {
      finishPane(pane, event.metrics || {});
    } else if (event.type === "stats") {
      pane.server = event.stats;
      if (pane.view === "metrics") renderMetrics(pane);
    }
  }

  const STATES = {
    sending: ["side.sending", "busy", null], waiting: ["side.waiting", "busy", null], stopping: ["side.stopping", "busy", null],
    thinking: ["side.thinkingState", "busy think", "thinking"], writing: ["side.writing", "busy", null],
    done: ["side.done", "ok", "done"], error: ["side.error", "bad", "error"], stopped: ["side.stopped", "", "stopped"],
  };
  function setState(pane, kind) {
    const [key, cls, symbol] = STATES[kind];
    pane.state.className = "pane-state " + cls;
    pane.state.title = t(key);
    pane.state.setAttribute("aria-label", t(key));
    pane.state.replaceChildren(symbol ? icon(symbol) : el("i", {class: "pulse"}));
  }

  function finishPane(pane, metrics) {
    if (pane.done) return;
    pane.done = true;
    pane.metrics = metrics;
    pane.error = metrics.error || null;
    if (metrics.first_thinking_s !== undefined && pane.firstThinking === null) pane.firstThinking = metrics.first_thinking_s;
    if (metrics.first_answer_s !== undefined && pane.firstAnswer === null) pane.firstAnswer = metrics.first_answer_s;
    if (metrics.connected_s !== undefined && pane.connected === null) pane.connected = metrics.connected_s;
    pane.dirty = true;
    setState(pane, pane.error ? "error" : metrics.stopped ? "stopped" : "done");
    paint(pane, performance.now());
    // Finished answers read from the top, unless the reader scrolled.
    if (pane.follow && pane.body.scrollTo) pane.body.scrollTo({top: 0, behavior: reduceMotion ? "auto" : "smooth"});
    if (pane.view === "metrics") renderMetrics(pane);
    if (S.ask && S.ask.panes.every((p) => p.done)) finishAll();
  }

  function startTicking() {
    if (S.ticking) return;
    S.ticking = true;
    let lastText = 0;
    const frame = (now) => {
      if (!S.ask) {
        S.ticking = false;
        return;
      }
      const renderText = now - lastText > 50;
      if (renderText) lastText = now;
      const scale = timeScale(now);
      for (const pane of S.ask.panes) {
        if (renderText || pane.done) paint(pane, now, scale);
        else paintTrack(pane, now, scale);
      }
      if (S.ask.panes.every((p) => p.done)) {
        for (const pane of S.ask.panes) paint(pane, now, scale);
        S.ticking = false;
        return;
      }
      requestAnimationFrame(frame);
    };
    requestAnimationFrame(frame);
  }

  const elapsed = (now) => (now - S.ask.started) / 1000;
  const paneTime = (pane, now) => (pane.done && pane.metrics && pane.metrics.total_s !== undefined ? pane.metrics.total_s : elapsed(now));
  const firstToken = (pane) => [pane.firstThinking, pane.firstAnswer].filter((v) => v !== null && v !== undefined).reduce((a, b) => Math.min(a, b), Infinity);

  // One shared time axis, so the tracks compare at a glance.
  function timeScale(now) {
    const longest = Math.max(1, ...S.ask.panes.map((p) => paneTime(p, now)));
    const steps = [1, 2, 3, 5, 8, 10, 15, 20, 30, 45, 60, 90, 120, 180, 300, 600];
    return steps.find((s) => s >= longest * 1.08) || Math.ceil(longest / 60) * 60;
  }

  function paintTrack(pane, now, scale) {
    const end = paneTime(pane, now);
    const first = firstToken(pane);
    const pct = (s) => `${Math.max(0, Math.min(100, (s / scale) * 100))}%`;
    const [send, wait, think, write] = pane.track.children;
    const connected = pane.connected ?? Math.min(end, first);
    const place = (seg, a, b) => {
      seg.style.left = pct(a);
      seg.style.width = b > a ? `calc(${pct(b)} - ${pct(a)})` : "0";
    };
    place(send, 0, Math.min(connected, end));
    place(wait, connected, Number.isFinite(first) ? first : end);
    if (pane.firstThinking !== null) place(think, pane.firstThinking, pane.firstAnswer ?? end);
    else place(think, 0, 0);
    if (pane.firstAnswer !== null) place(write, pane.firstAnswer, end);
    else place(write, 0, 0);
    pane.flag.hidden = !(pane.done && !pane.error);
    pane.flag.style.left = pct(end);
  }

  function paint(pane, now, scale = timeScale(now)) {
    paintTrack(pane, now, scale);
    const m = pane.metrics || {};
    const values = pane.stats.querySelectorAll(".stat-value");
    const first = firstToken(pane);
    values[0].textContent = Number.isFinite(first) ? secs(first) : pane.done ? "—" : "…";
    if (pane.done && m.tokens_per_s) values[1].textContent = Math.round(m.tokens_per_s) + "/s";
    else if (Number.isFinite(first) && !pane.done) {
      const writing = elapsed(now) - first;
      const estimate = (pane.text.length + pane.thinking.length) / 4;
      values[1].textContent = writing > 0.3 ? "≈" + Math.round(estimate / writing) + "/s" : "…";
    } else values[1].textContent = pane.done ? "—" : "…";
    values[2].textContent = secs(paneTime(pane, now));
    const cost = m.cost_usd;
    values[3].textContent = !pane.done ? "…" : cost === null || cost === undefined ? "—" : cost === 0 ? t("ai.free")
      : (cost * rate() < 0.01 ? "<0.01" : rupees(cost).replace("₹", ""));
    if (!pane.dirty) return;
    pane.dirty = false;
    if (pane.thinking) pane.thinkingBox.querySelector(".thinking-text").textContent = pane.thinking;
    if (pane.error) {
      pane.answer.replaceChildren(el("p", {class: "pane-error", text: pane.error}));
      if (pane.text) pane.answer.prepend(...markdown(pane.text));
    } else {
      pane.answer.replaceChildren(...markdown(pane.text));
      if (!pane.done && pane.firstAnswer !== null) pane.answer.append(el("span", {class: "caret", "aria-hidden": "true"}));
      if (!pane.text && pane.done) pane.answer.append(el("p", {class: "muted", text: t("side.noAnswer")}));
    }
    if (pane.follow) pane.body.scrollTop = pane.body.scrollHeight;
  }

  // A small, safe Markdown reader: paragraphs, lists, headings, code.
  function markdown(text) {
    const nodes = [];
    const inline = (line, into) => {
      const parts = line.split(/(`[^`]+`|\*\*[^*]+\*\*)/g);
      for (const part of parts) {
        if (!part) continue;
        if (part.startsWith("`") && part.endsWith("`") && part.length > 1) into.append(el("code", {text: part.slice(1, -1)}));
        else if (part.startsWith("**") && part.endsWith("**") && part.length > 3) into.append(el("strong", {text: part.slice(2, -2)}));
        else into.append(document.createTextNode(part));
      }
    };
    text.split("```").forEach((chunk, index) => {
      if (index % 2 === 1) {
        const newline = chunk.indexOf("\n");
        nodes.push(el("pre", {}, el("code", {text: (newline >= 0 ? chunk.slice(newline + 1) : chunk).replace(/\n$/, "")})));
        return;
      }
      // Line by line: runs of list items become a list, other lines a
      // paragraph, and blank lines end either.
      let para = null, list = null;
      for (const raw of chunk.split("\n")) {
        const line = raw.trim();
        if (!line) {
          para = null;
          list = null;
          continue;
        }
        const item = /^([-*•]|\d+[.)])\s+(.*)$/.exec(line);
        if (item) {
          if (!list) {
            list = el(/\d/.test(item[1]) ? "ol" : "ul");
            nodes.push(list);
            para = null;
          }
          const li = el("li");
          inline(item[2], li);
          list.append(li);
          continue;
        }
        list = null;
        const heading = /^#{1,6}\s+(.*)$/.exec(line);
        if (heading) {
          const h = el("p", {class: "md-h"});
          inline(heading[1], h);
          nodes.push(h);
          para = null;
          continue;
        }
        if (para) para.append(el("br"));
        else {
          para = el("p");
          nodes.push(para);
        }
        inline(line, para);
      }
    });
    return nodes;
  }

  /* ---------- every metric ---------- */

  function finishReason(reason) {
    const words = {stop: "side.finStop", length: "side.finLength", content_filter: "side.finFilter", tool_calls: "side.finTools", error: "side.finError"};
    return reason ? (words[reason] ? t(words[reason]) : reason) : "—";
  }

  function renderMetrics(pane) {
    const m = pane.metrics || {};
    const info = pane.m;
    const server = pane.server || {};
    const now = performance.now();
    const first = firstToken(pane);
    const total = paneTime(pane, now);
    const input = m.input_tokens, output = m.output_tokens;
    const used = input !== undefined && input !== null ? (input || 0) + (output || 0) : null;
    const share = used !== null && info.context ? `${((used / info.context) * 100).toFixed(used / info.context < 0.01 ? 2 : 1)}%` : "—";
    const inCost = input !== undefined && input !== null && info.input_usd !== null && info.input_usd !== undefined ? input * info.input_usd : null;
    const outCost = output !== undefined && output !== null && info.output_usd !== null && info.output_usd !== undefined ? output * info.output_usd : null;
    const perMillion = (usd) => (usd === null || usd === undefined ? "—" : usd === 0 ? t("ai.free") : rupees(usd * 1e6));
    const sections = [
      [t("side.secTiming"), [
        [t("side.mConnected"), secs(pane.connected)],
        [t("side.mFirstThought"), secs(pane.firstThinking)],
        [t("side.mFirstWord"), secs(pane.firstAnswer)],
        [t("side.mThinkingTime"), pane.firstThinking !== null ? secs((pane.firstAnswer ?? total) - pane.firstThinking) : "—"],
        [t("side.mWritingTime"), Number.isFinite(first) ? secs(total - first) : "—"],
        [t("side.mTotal"), secs(total)],
        [t("side.mPause"), m.longest_pause_s !== undefined ? secs(m.longest_pause_s) : "—"],
        [t("side.mServerLatency"), ms(server.latency_ms)],
        [t("side.mServerGen"), ms(server.generation_time_ms)],
      ]],
      [t("side.secTokens"), [
        [t("side.mInput"), tokens(input)],
        [t("side.mOutput"), tokens(output)],
        [t("side.mReasoning"), tokens(m.reasoning_tokens)],
        [t("side.mCached"), tokens(m.cached_tokens)],
        [t("side.mSpeed"), m.tokens_per_s ? t("side.tps", {n: m.tokens_per_s.toFixed(1)}) : "—"],
        [t("side.mChunks"), tokens(m.chunks)],
        [t("side.mWords"), m.answer_words !== undefined ? `${tokens(m.answer_words)} · ${tokens(m.answer_chars)}` : "—"],
        [t("side.mNative"), server.native_output_tokens !== undefined ? `${tokens(server.native_input_tokens)} / ${tokens(server.native_output_tokens)}` : "—"],
      ]],
      [t("side.secContext"), [
        [t("side.mWindow"), info.context ? tokens(info.context) : "—"],
        [t("side.mUsed"), used !== null ? `${tokens(used)} · ${share}` : "—"],
        [t("side.mMaxReply"), tokens(info.max_output)],
      ]],
      [t("side.secCost"), [
        [t("side.mCost"), m.cost_usd !== undefined && m.cost_usd !== null ? `${money(m.cost_usd)} · $${m.cost_usd.toFixed(m.cost_usd < 0.01 ? 5 : 3)}` : "—"],
        [t("side.mInputCost"), inCost !== null ? money(inCost) : "—"],
        [t("side.mOutputCost"), outCost !== null ? money(outCost) : "—"],
        [t("side.mPriceIn"), perMillion(info.input_usd)],
        [t("side.mPriceOut"), perMillion(info.output_usd)],
      ]],
      [t("side.secRoute"), [
        [t("side.mModel"), info.id],
        [t("side.mProvider"), m.provider || server.provider || "—"],
        [t("side.mFinish"), finishReason(m.finish_reason)],
        [t("side.mStatus"), m.http_status ? String(m.http_status) : "—"],
        [t("side.mGeneration"), m.generation_id || "—"],
      ]],
    ];
    const box = pane.metricsBox;
    box.replaceChildren(...sections.map(([title, rows]) => el("section", {class: "msec"},
      el("h3", {text: title}),
      el("dl", {}, rows.flatMap(([label, value]) => [el("dt", {text: label}), el("dd", {text: value})])))));
  }

  /* ---------- when everyone has answered ---------- */

  const WINS = {start: "side.winStart", finish: "side.winFinish", speed: "side.winWriter", cheap: "side.winCheap"};
  function badge(kind) {
    const node = kind === "cheap" ? el("span", {class: "win-badge", text: "₹"}) : el("span", {class: "win-badge" + (kind === "finish" ? " filled" : "")}, icon(kind === "finish" ? "flag" : kind));
    node.title = t(WINS[kind]);
    node.setAttribute("role", "img");
    node.setAttribute("aria-label", t(WINS[kind]));
    return node;
  }

  function finishAll() {
    const ok = S.ask.panes.filter((p) => !p.error && p.metrics && !p.metrics.stopped);
    for (const pane of S.ask.panes) pane.node.querySelector(".pane-badges").replaceChildren();
    const wins = [];
    if (ok.length >= 2) {
      const best = (score, kind, better) => {
        const scored = ok.filter((p) => Number.isFinite(score(p)));
        if (scored.length < 2) return;
        const top = scored.reduce((a, b) => (better(score(b), score(a)) ? b : a));
        if (scored.filter((p) => score(p) === score(top)).length > 1) return;
        wins.push([top, kind]);
      };
      best((p) => p.metrics.total_s, "finish", (a, b) => a < b);
      best((p) => p.metrics.tokens_per_s ?? NaN, "speed", (a, b) => a > b);
      best((p) => p.metrics.cost_usd ?? NaN, "cheap", (a, b) => a < b);
      best((p) => firstToken(p), "start", (a, b) => a < b);
    }
    wins.forEach(([pane, kind], index) => {
      const node = badge(kind);
      pane.node.querySelector(".pane-badges").append(node);
      pop(node, 120 + index * 90);
    });
    const spent = S.ask.panes.reduce((sum, p) => sum + ((p.metrics && p.metrics.cost_usd) || 0), 0);
    const summary = $("side-summary");
    summary.replaceChildren();
    for (const pane of S.ask.panes) {
      const mine = wins.filter(([p]) => p === pane).map(([, kind]) => kind);
      if (!mine.length) continue;
      summary.append(el("span", {class: "sum-item"}, ...mine.map(badge), el("span", {text: splitName(pane.m).short})));
    }
    summary.append(el("span", {class: "sum-item sum-total", title: t("side.tipTotal")}, icon("total"), el("span", {text: rupees(spent)})));
    summary.hidden = false;
    $("side-stop").hidden = true;
  }

  /* ---------- saving ---------- */

  async function save() {
    if (!S.ask) return;
    const lines = [`# ${t("side.title")}`, "", `**${t("side.promptLabel")}:** ${S.ask.prompt}`, ""];
    for (const pane of S.ask.panes) {
      const m = pane.metrics || {};
      lines.push(`## ${splitName(pane.m).short} (${pane.id})`, "");
      lines.push(`| ${t("side.firstToken")} | ${t("side.speed")} | ${t("side.time")} | ${t("side.cost")} | ${t("side.mInput")} | ${t("side.mOutput")} |`);
      lines.push("|---|---|---|---|---|---|");
      lines.push(`| ${secs(firstToken(pane))} | ${m.tokens_per_s ? t("side.tps", {n: m.tokens_per_s.toFixed(1)}) : "—"} | ${secs(m.total_s)} | ${money(m.cost_usd)} | ${tokens(m.input_tokens)} | ${tokens(m.output_tokens)} |`, "");
      lines.push(pane.error ? `> ${pane.error}` : pane.text, "");
    }
    try {
      const where = await bridge.saveText(`side-by-side-${new Date().toISOString().slice(0, 16).replace(/[:T]/g, "-")}.md`, lines.join("\n"));
      if (where) setStatus(t("side.saved"));
    } catch (error) {
      setStatus(error.message);
    }
  }

  /* ---------- wiring ---------- */

  async function render() {
    if (!S.ask || $("side-run").hidden) renderKey();
    renderPicks();
    if (!S.loaded) {
      S.loaded = true;
      await Promise.all([loadStatus(), loadModels()]);
    }
    if ($("side-run").hidden) $("side-prompt").focus();
    fitStage();
  }

  function wire() {
    $("side-composer").addEventListener("submit", ask);
    $("side-prompt").addEventListener("keydown", (event) => {
      if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) ask(event);
    });
    $("side-key-save").addEventListener("click", saveKey);
    $("side-key-input").addEventListener("keydown", (event) => { if (event.key === "Enter") saveKey(); });
    $("side-key-link").addEventListener("click", (event) => {
      event.preventDefault();
      bridge.openExternal("https://openrouter.ai/keys");
    });
    $("tray-done").addEventListener("click", closeTray);
    $("side-tray").addEventListener("click", (event) => { if (event.target === $("side-tray")) closeTray(); });
    $("tray-search").addEventListener("input", (event) => { S.query = event.target.value; renderTray(); });
    $("tray-free").addEventListener("change", (event) => { S.onlyFree = event.target.checked; renderTray(); });
    document.addEventListener("keydown", (event) => { if (event.key === "Escape") closeTray(); });
    $("side-stop").addEventListener("click", stop);
    $("side-new").addEventListener("click", newQuestion);
    $("side-save").addEventListener("click", save);
    for (const button of document.querySelectorAll("#side-view button")) {
      button.addEventListener("click", () => setView(button.dataset.view));
    }
    window.addEventListener("resize", fitStage);
    bridge.onProgress(onEvent);
    document.addEventListener("km-lang", () => {
      if (state.screen !== "side") return;
      renderPicks();
      renderKey();
      if (S.ask) for (const pane of S.ask.panes) if (pane.view === "metrics") renderMetrics(pane);
    });
  }

  document.addEventListener("DOMContentLoaded", wire);
  return {render, state: S};
})();

function renderSide() {
  return sideBySide.render();
}
