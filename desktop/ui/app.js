"use strict";
const state = {
  catalog: null,
  selected: new Set(["oracle", "echo"]),
  pack: "india-pilot",
  cap: 1,
  repeats: 1,
  estimate: null,
  runId: null,
  dashboard: null,
  done: 0,
  total: 1,
  busy: false,
};

const $ = (id) => document.getElementById(id);

function showNotice(message) {
  const box = $("notice");
  box.hidden = !message;
  box.textContent = message || "";
}

function screen(name) {
  for (const section of document.querySelectorAll(".screen")) section.hidden = section.id !== name;
  if (name !== "welcome") showNotice("");
  if (name === "keys") renderProviders();
  if (name === "models") renderModels();
  if (name === "pack") renderPacks();
  if (name === "cost") refreshEstimate();
}

function money(usd, rate) {
  if (usd === null || usd === undefined) return "—";
  const rupees = (usd * rate).toLocaleString("en-IN", {style: "currency", currency: "INR", maximumFractionDigits: 2});
  const dollars = usd.toLocaleString("en-US", {style: "currency", currency: "USD", maximumFractionDigits: 4});
  return rupees + " · " + dollars;
}

function labelOf(item, key) {
  if (kmLang === "hi" && item[key + "_hi"]) return item[key + "_hi"];
  return item[key];
}

function renderProviders() {
  const root = $("provider-list");
  root.replaceChildren();
  const keys = state.catalog?.keys || {};
  for (const provider of state.catalog?.providers || []) {
    const card = document.createElement("article");
    card.className = "card provider";
    const head = document.createElement("div");
    head.className = "provider-head";
    const title = document.createElement("h2");
    title.textContent = provider.name;
    head.append(title);
    if (provider.signup) {
      const link = document.createElement("a");
      link.href = provider.signup;
      link.textContent = t("keys.get");
      link.target = "_blank";
      link.rel = "noopener";
      link.addEventListener("click", (event) => {
        event.preventDefault();
        bridge.openExternal(provider.signup);
      });
      head.append(link);
    }
    const blurb = document.createElement("p");
    blurb.textContent = labelOf(provider, "blurb");
    const row = document.createElement("div");
    row.className = "key-row";
    const input = document.createElement("input");
    input.type = "password";
    input.autocomplete = "off";
    input.spellcheck = false;
    input.setAttribute("aria-label", provider.name);
    input.placeholder = t("keys.placeholder");
    const save = document.createElement("button");
    save.type = "button";
    save.className = "secondary";
    save.textContent = keys[provider.id]?.saved ? t("keys.forget") : t("keys.save");
    const status = document.createElement("p");
    status.className = "saved";
    status.textContent = keys[provider.id]?.saved ? (bridge.isTauri() ? t("keys.saved") : t("keys.session")) : "";
    save.addEventListener("click", async () => {
      showNotice("");
      try {
        const where = await bridge.saveKey(provider.id, keys[provider.id]?.saved && !input.value ? "" : input.value.trim());
        input.value = "";
        state.catalog = await bridge.call("catalog");
        status.textContent = where === "keychain" ? t("keys.saved") : t("keys.session");
        if (!state.catalog.keys[provider.id]?.saved) status.textContent = "";
        save.textContent = state.catalog.keys[provider.id]?.saved ? t("keys.forget") : t("keys.save");
      } catch (error) {
        showNotice(error.message);
      }
    });
    row.append(input, save);
    card.append(head, blurb, row, status);
    root.append(card);
  }
}

function modelButton(model) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "card" + (model.spec && state.selected.has(model.spec) ? " selected" : "");
  button.disabled = !model.runnable;
  const title = document.createElement("h3");
  title.textContent = labelOf(model, "name");
  const blurb = document.createElement("p");
  blurb.textContent = labelOf(model, "blurb");
  const meta = document.createElement("div");
  meta.className = "meta";
  const price = document.createElement("span");
  if (model.free) price.textContent = "₹0 · $0";
  else if (model.price_usd) {
    price.textContent = "$" + model.price_usd.input + " in · $" + model.price_usd.output + " out / MTok";
  } else price.textContent = t("models.later");
  const flag = document.createElement("span");
  flag.textContent = model.suggested ? t("common.suggested") : "";
  meta.append(price, flag);
  button.append(title, blurb, meta);
  if (model.runnable) {
    button.addEventListener("click", () => {
      if (state.selected.has(model.spec)) state.selected.delete(model.spec);
      else state.selected.add(model.spec);
      renderModels();
    });
  }
  return button;
}

function renderModels() {
  const root = $("model-list");
  root.replaceChildren();
  const groups = [
    ["models.practice", (model) => model.free],
    ["models.priced", (model) => model.runnable && !model.free],
    ["models.later", (model) => !model.runnable],
  ];
  for (const [key, match] of groups) {
    const models = (state.catalog?.models || []).filter(match);
    if (!models.length) continue;
    const heading = document.createElement("h2");
    heading.className = "group-title";
    heading.textContent = t(key);
    const grid = document.createElement("div");
    grid.className = "grid";
    for (const model of models) grid.append(modelButton(model));
    root.append(heading, grid);
  }
}

function renderPacks() {
  const root = $("pack-list");
  root.replaceChildren();
  for (const pack of state.catalog?.packs || []) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "card" + (state.pack === pack.id ? " selected" : "");
    const title = document.createElement("h3");
    title.textContent = labelOf(pack, "name");
    const blurb = document.createElement("p");
    blurb.textContent = labelOf(pack, "summary");
    const meta = document.createElement("div");
    meta.className = "meta";
    const badge = document.createElement("span");
    badge.textContent = pack.badge + " · " + pack.tasks + " " + t("pack.tasks");
    const chips = document.createElement("span");
    chips.textContent = "Unvalidated pilot";
    meta.append(badge, chips);
    button.append(title, blurb, meta);
    button.addEventListener("click", () => {
      state.pack = pack.id;
      renderPacks();
    });
    root.append(button);
  }
}

function drawGauge(fraction) {
  const svg = $("gauge");
  const clamped = Math.max(0, Math.min(1, fraction || 0));
  const cx = 120, cy = 118, radius = 78;
  const point = (part, r) => {
    const angle = (210 - 240 * part) * Math.PI / 180;
    return [cx + r * Math.cos(angle), cy - r * Math.sin(angle)];
  };
  const arc = (from, to) => {
    const [x0, y0] = point(from, radius);
    const [x1, y1] = point(to, radius);
    const large = (to - from) * 240 > 180 ? 1 : 0;
    return `M${x0.toFixed(1)} ${y0.toFixed(1)} A${radius} ${radius} 0 ${large} 1 ${x1.toFixed(1)} ${y1.toFixed(1)}`;
  };
  svg.replaceChildren();
  const ns = "http://www.w3.org/2000/svg";
  const path = (d, className) => {
    const el = document.createElementNS(ns, "path");
    el.setAttribute("d", d);
    el.setAttribute("class", className);
    return el;
  };
  svg.append(path(arc(0, 1), "band"));
  if (clamped > 0.004) svg.append(path(arc(0, clamped), "sweep"));
  const [x2, y2] = point(clamped, radius - 22);
  const needle = document.createElementNS(ns, "line");
  needle.setAttribute("x1", String(cx));
  needle.setAttribute("y1", String(cy));
  needle.setAttribute("x2", x2.toFixed(1));
  needle.setAttribute("y2", y2.toFixed(1));
  needle.setAttribute("class", "needle");
  needle.setAttribute("stroke", "currentColor");
  needle.setAttribute("stroke-width", "3");
  svg.append(needle);
  svg.setAttribute("aria-label", `${state.done} of ${state.total}`);
}

function renderProgressReadout() {
  $("gauge-read").textContent = state.done + " / " + state.total;
  drawGauge(state.total ? state.done / state.total : 0);
}

async function refreshEstimate() {
  $("confirm").checked = false;
  $("run").disabled = true;
  if (!state.selected.size) {
    showNotice(t("models.title"));
    return;
  }
  try {
    state.estimate = await bridge.call("estimate", {
      suite: state.pack,
      runners: [...state.selected],
      repeats: state.repeats,
      max_usd: state.cap,
    });
    const rate = state.estimate.usd_to_inr;
    $("cost-hero").replaceChildren();
    const figure = document.createElement("div");
    const caption = document.createElement("span");
    caption.className = "muted";
    caption.textContent = "Worst case";
    const strong = document.createElement("strong");
    strong.textContent = money(state.estimate.worst_case_usd, rate);
    figure.append(caption, strong);
    const cap = document.createElement("div");
    const capLabel = document.createElement("span");
    capLabel.className = "muted";
    capLabel.textContent = t("cost.cap");
    const capValue = document.createElement("strong");
    capValue.textContent = money(state.estimate.cap_usd, rate);
    cap.append(capLabel, capValue);
    $("cost-hero").append(figure, cap);
    const table = document.createElement("table");
    const head = document.createElement("tr");
    for (const name of ["", "Worst case", "Status"]) head.append(Object.assign(document.createElement("th"), {textContent: name}));
    table.append(head);
    for (const row of state.estimate.runners) {
      const line = document.createElement("tr");
      const cells = [
        row.label,
        row.worst_case_usd === null ? "—" : money(row.worst_case_usd, rate),
        row.ready ? "Ready" : (row.plain_problems[0] || "Not ready"),
      ];
      for (const text of cells) line.append(Object.assign(document.createElement("td"), {textContent: text}));
      table.append(line);
    }
    $("cost-table").replaceChildren(table);
    $("confirm-label").textContent = state.estimate.paid ? t("confirm.paid") : t("confirm.free");
    if (!state.estimate.ok) showNotice(state.estimate.plain_problems[0] || state.estimate.problems[0] || "");
    else showNotice("");
    syncRunButton();
  } catch (error) {
    showNotice(error.message);
  }
}

function syncRunButton() {
  $("run").disabled = state.busy || !$("confirm").checked || !state.estimate?.ok;
}

function addAttempt(event) {
  const list = $("attempt-log");
  const item = document.createElement("li");
  const name = document.createElement("span");
  name.textContent = (event.title || event.task_id) + " · " + (event.runner_label || event.runner);
  const mark = document.createElement("span");
  mark.className = "mark";
  if (event.type === "asking") mark.textContent = "…";
  else if (event.passed === true) mark.textContent = "RIGHT";
  else if (event.passed === false) mark.textContent = "WRONG";
  else if (event.status === "skipped") mark.textContent = "SKIPPED";
  else mark.textContent = (event.status || "").toUpperCase();
  item.append(name, mark);
  list.append(item);
  list.scrollTop = list.scrollHeight;
}

async function startRun() {
  if ($("run").disabled) return;
  state.busy = true;
  state.done = 0;
  state.total = state.estimate?.attempts || 1;
  state.runId = null;
  $("attempt-log").replaceChildren();
  $("open-results").disabled = true;
  $("cancel").disabled = false;
  renderProgressReadout();
  screen("progress");
  try {
    const result = await bridge.call("run", {
      suite: state.pack,
      runners: [...state.selected],
      repeats: state.repeats,
      max_usd: state.cap,
      confirmed: true,
      seed: 0,
    });
    state.runId = result.run_id;
    state.dashboard = result.dashboard_url;
    $("open-results").disabled = false;
    $("cancel").disabled = true;
  } catch (error) {
    showNotice(error.message);
    $("cancel").disabled = true;
  } finally {
    state.busy = false;
  }
}

function openResults() {
  if (!state.dashboard) return;
  const theme = window.kmTheme ? window.kmTheme() : "system";
  const url = new URL(state.dashboard);
  url.searchParams.set("theme", theme);
  $("dashboard").src = url.toString();
  screen("results");
}

async function saveReport(filename) {
  const report = await bridge.call("report", {run_id: state.runId});
  await bridge.saveText(filename, JSON.stringify(report, null, 2));
}

document.addEventListener("DOMContentLoaded", async () => {
  applyI18n();
  for (const button of document.querySelectorAll("[data-lang]")) {
    button.addEventListener("click", () => setLang(button.dataset.lang));
  }
  document.addEventListener("km-lang", () => {
    renderProviders();
    renderModels();
    renderPacks();
    if (!$("cost").hidden && state.estimate) $("confirm-label").textContent = state.estimate.paid ? t("confirm.paid") : t("confirm.free");
  });
  $("start").addEventListener("click", () => {
    try { localStorage.setItem("km.desktop.seen", "1"); } catch { /* Onboarding still advances. */ }
    screen("keys");
  });
  $("keys-link").addEventListener("click", () => screen("keys"));
  $("home-link").addEventListener("click", (event) => { event.preventDefault(); screen("welcome"); });
  $("keys-back").addEventListener("click", () => screen("welcome"));
  $("keys-skip").addEventListener("click", () => screen("models"));
  $("keys-next").addEventListener("click", () => screen("models"));
  $("models-next").addEventListener("click", () => {
    if (!state.selected.size) { showNotice(t("models.body")); return; }
    screen("pack");
  });
  $("pack-next").addEventListener("click", () => screen("cost"));
  for (const button of document.querySelectorAll("[data-goto]")) {
    button.addEventListener("click", () => screen(button.dataset.goto));
  }
  $("cap").addEventListener("change", () => {
    state.cap = Math.max(0, Number($("cap").value) || 0);
    refreshEstimate();
  });
  $("repeats").addEventListener("change", () => {
    state.repeats = Number($("repeats").value);
    refreshEstimate();
  });
  $("confirm").addEventListener("change", syncRunButton);
  $("run").addEventListener("click", startRun);
  $("cancel").addEventListener("click", () => bridge.call("cancel").catch((error) => showNotice(error.message)));
  $("open-results").addEventListener("click", openResults);
  $("save-json").addEventListener("click", () => saveReport((state.runId || "results") + ".json").catch((error) => showNotice(error.message)));
  $("export-json").addEventListener("click", () => saveReport((state.runId || "results") + "-export.json").catch((error) => showNotice(error.message)));
  $("share-card").addEventListener("click", async () => {
    try {
      const card = await bridge.call("share_card", {run_id: state.runId});
      try { await navigator.clipboard.writeText(card.summary); } catch { /* The file is still saved. */ }
      await bridge.saveText("krutrim-mulyankan-card.html", card.html);
    } catch (error) {
      showNotice(error.message);
    }
  });
  $("again").addEventListener("click", () => screen("models"));
  bridge.onProgress((event) => {
    if (event.type === "start") {
      state.total = event.total || state.total;
      renderProgressReadout();
    } else if (event.type === "asking") {
      addAttempt(event);
    } else if (event.type === "attempt") {
      state.done = event.index;
      renderProgressReadout();
      addAttempt(event);
    } else if (event.type === "done") {
      state.done = state.total;
      renderProgressReadout();
    }
  });
  try {
    await bridge.init();
    let lastError = null;
    for (let attempt = 0; attempt < 20; attempt += 1) {
      try {
        state.catalog = await bridge.call("catalog");
        lastError = null;
        break;
      } catch (error) {
        lastError = error;
        await new Promise((resolve) => setTimeout(resolve, 250));
      }
    }
    if (lastError) throw lastError;
    state.cap = state.catalog.default_cap_usd ?? 1;
    $("cap").value = String(state.cap);
    if (!state.catalog.packs.some((pack) => pack.id === state.pack) && state.catalog.packs[0]) {
      state.pack = state.catalog.packs[0].id;
    }
    let seen = null;
    try { seen = localStorage.getItem("km.desktop.seen"); } catch { /* Welcome stays available. */ }
    screen(seen ? "models" : "welcome");
  } catch (error) {
    showNotice(error.message);
  }
});
