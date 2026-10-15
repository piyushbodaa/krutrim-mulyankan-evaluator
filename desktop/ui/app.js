"use strict";
/* Three steps for people who have never heard the word "eval":
   pick questions (or write your own) -> pick AIs -> start, then a report card. */
const MAX_AIS = 6;
const PICKS_KEY = "km.desktop.picks";
const PROVIDERS = {
  anthropic: {family: "Claude", maker: "Anthropic"},
  openai: {family: "GPT", maker: "OpenAI"},
  gemini: {family: "Gemini", maker: "Google"},
  xai: {family: "Grok", maker: "xAI"},
  openrouter: {family: "OpenRouter", maker: "OpenRouter"},
};

const state = {
  catalog: null,
  pack: "india-pilot",
  selected: new Set(["oracle", "echo"]),
  costs: null,
  costsFor: "",
  repeats: 1,
  capInr: null,
  estimate: null,
  agreed: false,
  busy: false,
  run: null,
  runId: null,
  report: null,
  draft: null,
  showOlder: false,
  armedDelete: null,
  modalProvider: null,
  screen: "home",
};

const $ = (id) => document.getElementById(id);

/* Small DOM helper: el("p", {class: "x", text: "y", onclick: fn}, child...) */
function el(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (value === undefined || value === null || value === false) continue;
    if (key === "class") node.className = value;
    else if (key === "text") node.textContent = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else if (key in node && typeof value !== "string") node[key] = value;
    else node.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}

function svg(paths, className = "icon") {
  const ns = "http://www.w3.org/2000/svg";
  const node = document.createElementNS(ns, "svg");
  node.setAttribute("viewBox", "0 0 24 24");
  node.setAttribute("class", className);
  node.setAttribute("aria-hidden", "true");
  node.innerHTML = paths;
  return node;
}

const ICON_RIGHT = '<circle cx="12" cy="12" r="10" class="fill"/><path d="M7.5 12.5l3 3 6-6.5" class="on-fill"/>';
const ICON_WRONG = '<circle cx="12" cy="12" r="9.5"/><path d="M8.5 8.5l7 7M15.5 8.5l-7 7"/>';
const ICON_DASH = '<circle cx="12" cy="12" r="9.5" stroke-dasharray="3 3"/><path d="M8 12h8"/>';

function showNotice(message) {
  const box = $("notice");
  box.hidden = !message;
  box.textContent = message || "";
  if (message) box.scrollIntoView({block: "nearest"});
}

function rate() {
  return state.catalog?.fx?.usd_to_inr || 88;
}

function rupees(usd) {
  if (usd === null || usd === undefined) return "—";
  const inr = usd * rate();
  if (inr > 0 && inr < 0.01) return kmLang === "hi" ? "₹0.01 से कम" : "under ₹0.01";
  return inr.toLocaleString("en-IN", {style: "currency", currency: "INR", minimumFractionDigits: inr < 100 ? 2 : 0, maximumFractionDigits: 2});
}

function duration(seconds) {
  const s = Math.max(0, seconds || 0);
  const hi = kmLang === "hi";
  if (s < 1) return hi ? "1 सेकंड से कम" : "under 1 s";
  if (s < 60) return (s < 10 ? s.toFixed(1) : Math.round(s)) + (hi ? " सेकंड" : " s");
  const minutes = Math.floor(s / 60);
  const rest = Math.round(s % 60);
  return hi ? `${minutes} मिनट ${rest} सेकंड` : `${minutes} min ${rest} s`;
}

function questionsText(n) {
  return n === 1 ? t("q.one") : t("q.many", {n});
}

function savePicks() {
  try {
    localStorage.setItem(PICKS_KEY, JSON.stringify({pack: state.pack, selected: [...state.selected]}));
  } catch { /* Picks still work for this session. */ }
}

function loadPicks() {
  try {
    const saved = JSON.parse(localStorage.getItem(PICKS_KEY) || "null");
    if (saved && typeof saved.pack === "string") state.pack = saved.pack;
    if (saved && Array.isArray(saved.selected) && saved.selected.length) state.selected = new Set(saved.selected);
  } catch { /* Defaults apply. */ }
}

/* ---------- models in everyday words ---------- */

function modelInfo(model) {
  const provider = model.provider;
  const match = /^claude-([a-z]+)-(\d+)(?:-(\d+))?$/.exec(model.model || "");
  if (model.free) {
    return {spec: model.spec, provider, label: labelOf(model, "name"), blurb: labelOf(model, "blurb"), family: "demo", version: [0, 0]};
  }
  if (match) {
    const family = match[1];
    const label = `Claude ${family[0].toUpperCase()}${family.slice(1)} ${match[2]}${match[3] ? "." + match[3] : ""}`;
    const blurbKey = ["haiku", "sonnet", "opus", "fable"].includes(family) ? "tier." + family : "tier.other";
    return {spec: model.spec, provider, label, blurb: t(blurbKey), family, version: [Number(match[2]), Number(match[3] || 0)]};
  }
  return {spec: model.spec, provider, label: model.name, blurb: t("tier.other"), family: model.model || model.name, version: [0, 0]};
}

function prettyName(spec, fallback) {
  const model = (state.catalog?.models || []).find((m) => m.spec === spec);
  return model ? modelInfo(model).label : (fallback || spec);
}

function labelOf(item, key) {
  if (kmLang === "hi" && item[key + "_hi"]) return item[key + "_hi"];
  return item[key];
}

function providerOf(spec) {
  const match = /^model:([^/]+)\//.exec(spec || "");
  return match ? match[1] : null;
}

function hasKey(provider) {
  return Boolean(state.catalog?.keys?.[provider]?.saved);
}

/* ---------- navigation ---------- */

function go(name) {
  state.screen = name;
  for (const section of document.querySelectorAll(".screen")) section.hidden = section.id !== name;
  showNotice("");
  state.armedDelete = null;
  for (const stepper of document.querySelectorAll(".stepper")) renderStepper(stepper);
  const renderers = {home: renderHome, questions: renderQuestions, builder: renderBuilder, ais: renderAis, start: renderStart, accounts: renderAccounts};
  if (renderers[name]) renderers[name]();
  window.scrollTo(0, 0);
  const heading = document.querySelector(`#${name} h1`);
  if (heading) heading.focus({preventScroll: true});
}

function renderStepper(list) {
  const current = Number(list.dataset.step);
  list.replaceChildren();
  list.setAttribute("aria-label", t("step.of", {n: current}));
  ["step.questions", "step.ais", "step.start"].forEach((key, index) => {
    const n = index + 1;
    const item = el("li", {class: n === current ? "on" : n < current ? "done" : ""},
      el("span", {class: "dot", text: n < current ? "✓" : String(n)}), el("span", {text: t(key)}));
    if (n === current) item.setAttribute("aria-current", "step");
    list.append(item);
  });
}

/* ---------- home ---------- */

async function renderHome() {
  let runs = [];
  try { runs = await bridge.call("list_runs"); } catch { /* No past runs yet. */ }
  const list = $("past-list");
  list.replaceChildren();
  $("past").hidden = !runs.length;
  for (const run of runs.slice(0, 6)) {
    const when = new Date(run.started_at);
    const date = isNaN(when) ? "" : when.toLocaleString(kmLang === "hi" ? "hi-IN" : "en-IN", {day: "numeric", month: "short", hour: "numeric", minute: "2-digit"});
    const count = (run.runners || []).length;
    const ais = count === 1 ? prettyName(run.runners[0]) : `${count} ${kmLang === "hi" ? "एआई" : "AIs"}`;
    const spent = run.spent_usd > 0 ? rupees(run.spent_usd) : t("ai.free");
    list.append(el("li", {},
      el("div", {}, el("strong", {text: run.suite_label || run.suite}), el("span", {class: "muted small", text: [date, ais, spent].filter(Boolean).join(" · ")})),
      el("button", {class: "secondary", type: "button", text: t("home.open"), onclick: () => showResults(run.id)})));
  }
}

/* ---------- step 1: questions ---------- */

function packCard(pack) {
  const selected = state.pack === pack.id;
  const input = el("input", {type: "radio", name: "pack", value: pack.id, checked: selected,
    onchange: () => { state.pack = pack.id; savePicks(); renderQuestions(); }});
  const label = el("label", {class: "pick-main"}, input,
    el("span", {class: "pick-text"},
      pack.recommended ? el("span", {class: "badge", text: t("q.recommended")}) : null,
      el("span", {class: "pick-title", text: labelOf(pack, "name")}),
      pack.mine ? null : el("span", {class: "pick-blurb", text: labelOf(pack, "summary")}),
      el("span", {class: "pick-meta", text: questionsText(pack.tasks)})));
  const card = el("div", {class: "card pick" + (selected ? " selected" : "")}, label);
  if (pack.mine) {
    const armed = state.armedDelete === pack.id;
    card.append(el("div", {class: "card-tools"},
      el("button", {class: "link", type: "button", text: t("q.edit"), onclick: () => editTest(pack.id)}),
      el("button", {class: "link" + (armed ? " danger" : ""), type: "button", text: armed ? t("q.deleteSure") : t("q.delete"),
        onclick: () => deleteTest(pack.id)})));
  }
  return card;
}

function renderQuestions() {
  const root = $("question-sets");
  root.replaceChildren();
  const packs = state.catalog?.packs || [];
  if (!packs.some((p) => p.id === state.pack) && packs[0]) state.pack = packs[0].id;
  const mine = packs.filter((p) => p.mine);
  const ready = packs.filter((p) => !p.mine);
  root.append(el("h2", {class: "group-title", text: t("q.mine")}));
  const mineGrid = el("div", {class: "grid"});
  mineGrid.append(el("button", {class: "card new-card", type: "button", onclick: () => newTest()},
    svg('<path d="M12 5v14M5 12h14"/>', "icon icon-lg"),
    el("span", {class: "pick-title", text: t("q.new")}),
    el("span", {class: "pick-blurb", text: t("q.newSub")})));
  for (const pack of mine) mineGrid.append(packCard(pack));
  root.append(mineGrid);
  root.append(el("h2", {class: "group-title", text: t("q.ready")}));
  const readyGrid = el("div", {class: "grid"});
  for (const pack of ready) readyGrid.append(packCard(pack));
  root.append(readyGrid);
}

async function deleteTest(id) {
  if (state.armedDelete !== id) {
    state.armedDelete = id;
    renderQuestions();
    return;
  }
  state.armedDelete = null;
  try {
    await bridge.call("delete_test", {id});
    await refreshCatalog();
    renderQuestions();
    showNotice(t("q.deleted"));
  } catch (error) {
    showNotice(error.message);
  }
}

/* ---------- write your own test ---------- */

function blankQuestion() {
  return {question: "", answer: "", check: "includes"};
}

function newTest() {
  state.draft = {id: null, name: "", questions: [blankQuestion(), blankQuestion(), blankQuestion()]};
  go("builder");
}

async function editTest(id) {
  try {
    const test = await bridge.call("get_test", {id});
    state.draft = {id: test.id, name: test.name, questions: test.questions.map((q) => ({...q}))};
    go("builder");
  } catch (error) {
    showNotice(error.message);
  }
}

function renderBuilder() {
  if (!state.draft) state.draft = {id: null, name: "", questions: [blankQuestion()]};
  const draft = state.draft;
  $("builder-title").textContent = draft.id ? t("b.titleEdit") : t("b.titleNew");
  const name = $("test-name");
  name.value = draft.name;
  name.oninput = () => { draft.name = name.value; };
  const examples = $("example-list");
  examples.replaceChildren();
  for (const example of EXAMPLES[kmLang] || EXAMPLES.en) {
    examples.append(el("button", {class: "chip-button", type: "button", text: example.name, onclick: () => useExample(example)}));
  }
  renderEditor();
}

function useExample(example) {
  const draft = state.draft;
  const filled = draft.questions.filter((q) => q.question.trim() || q.answer.trim());
  draft.questions = [...filled, ...example.questions.map((q) => ({...q}))];
  if (!draft.name.trim()) {
    draft.name = example.name;
    $("test-name").value = example.name;
  }
  renderEditor();
}

function renderEditor() {
  const list = $("question-editor");
  list.replaceChildren();
  const checks = [["includes", "b.includes", "b.includesHelp"], ["number", "b.number", "b.numberHelp"], ["exact", "b.exact", "b.exactHelp"]];
  state.draft.questions.forEach((item, index) => {
    const n = index + 1;
    const qid = "q-" + index;
    const question = el("textarea", {id: qid, rows: 2, maxlength: 4000, placeholder: t("b.questionPh"),
      oninput: (event) => { item.question = event.target.value; }});
    question.value = item.question;
    const answer = el("input", {id: qid + "-a", type: "text", maxlength: 300, autocomplete: "off", placeholder: t("b.answerPh"),
      oninput: (event) => { item.answer = event.target.value; }});
    answer.value = item.answer;
    const help = el("p", {class: "muted small check-help", text: t(checks.find((c) => c[0] === item.check)?.[2] || "b.includesHelp")});
    const options = el("div", {class: "seg-choice", role: "radiogroup", "aria-label": t("b.check")});
    for (const [value, labelKey, helpKey] of checks) {
      const radio = el("input", {type: "radio", name: qid + "-check", value, checked: item.check === value,
        onchange: () => { item.check = value; help.textContent = t(helpKey); }});
      options.append(el("label", {}, radio, el("span", {text: t(labelKey)})));
    }
    const remove = el("button", {class: "link", type: "button", text: t("b.remove"), "aria-label": `${t("b.remove")} ${t("b.question", {n})}`,
      onclick: () => {
        state.draft.questions.splice(index, 1);
        if (!state.draft.questions.length) state.draft.questions.push(blankQuestion());
        renderEditor();
      }});
    list.append(el("li", {class: "card editor-item"},
      el("div", {class: "editor-head"}, el("label", {for: qid, class: "editor-label", text: t("b.question", {n})}), remove),
      question,
      el("div", {class: "editor-row"},
        el("label", {class: "field"}, el("span", {text: t("b.answer")}), answer),
        el("div", {class: "field"}, el("span", {text: t("b.check")}), options)),
      help));
  });
}

async function saveDraft() {
  const draft = state.draft;
  draft.name = $("test-name").value;
  const questions = draft.questions.filter((q) => q.question.trim() || q.answer.trim());
  if (!draft.name.trim()) { showNotice(t("b.needName")); $("test-name").focus(); return; }
  for (const [index, q] of questions.entries()) {
    if (!q.question.trim()) { showNotice(t("b.needQuestion", {n: index + 1})); return; }
    if (!q.answer.trim()) { showNotice(t("b.needAnswer", {n: index + 1})); return; }
  }
  if (!questions.length) { showNotice(t("b.needQuestion", {n: 1})); return; }
  try {
    const saved = await bridge.call("save_test", {id: draft.id, name: draft.name, questions});
    await refreshCatalog();
    state.pack = saved.id;
    state.costsFor = "";
    state.draft = null;
    savePicks();
    go("ais");
  } catch (error) {
    showNotice(error.message);
  }
}

/* ---------- step 2: AIs ---------- */

async function loadCosts() {
  const key = state.pack + "/" + state.repeats;
  if (state.costsFor === key && state.costs) return;
  try {
    state.costs = await bridge.call("pack_costs", {suite: state.pack, repeats: state.repeats});
    state.costsFor = key;
  } catch {
    state.costs = null;
  }
}

function aiCard(info) {
  const selected = state.selected.has(info.spec);
  const full = !selected && state.selected.size >= MAX_AIS;
  const cost = state.costs?.costs?.[info.spec];
  const input = el("input", {type: "checkbox", checked: selected, disabled: full,
    onchange: () => {
      if (state.selected.has(info.spec)) state.selected.delete(info.spec);
      else if (state.selected.size < MAX_AIS) state.selected.add(info.spec);
      savePicks();
      renderAis();
    }});
  const price = info.family === "demo" ? t("ai.free") : (cost === undefined || cost === null ? "" : t("ai.upTo", {money: rupees(cost)}));
  const label = el("label", {class: "pick-main", title: full ? t("ai.max") : null}, input,
    el("span", {class: "pick-text"},
      el("span", {class: "pick-title", text: info.label}),
      el("span", {class: "pick-blurb", text: info.blurb}),
      price ? el("span", {class: "pick-meta", text: price}) : null));
  const card = el("div", {class: "card pick" + (selected ? " selected" : "")}, label);
  if (info.provider && PROVIDERS[info.provider] && !hasKey(info.provider)) {
    card.append(el("div", {class: "card-tools"},
      el("span", {class: "muted small", text: t("ai.needsKey", {maker: PROVIDERS[info.provider].maker})}),
      el("button", {class: "link", type: "button", text: t("ai.connect"), onclick: () => openConnect(info.provider)})));
  }
  return card;
}

async function renderAis() {
  await loadCosts();
  const root = $("ai-list");
  root.replaceChildren();
  const models = state.catalog?.models || [];
  const infos = models.filter((m) => m.runnable).map(modelInfo);
  const known = new Set(infos.map((i) => i.spec));
  for (const spec of [...state.selected]) if (!known.has(spec)) state.selected.delete(spec);
  $("picked-count").textContent = t("ai.picked", {n: state.selected.size});

  const demo = infos.filter((i) => i.family === "demo");
  root.append(el("h2", {class: "group-title", text: t("ai.demo")}));
  root.append(el("div", {class: "grid"}, demo.map(aiCard)));

  const providers = [...new Set(infos.filter((i) => i.family !== "demo").map((i) => i.provider))];
  for (const provider of providers) {
    const mine = infos.filter((i) => i.provider === provider && i.family !== "demo");
    const newest = new Map();
    for (const info of mine) {
      const best = newest.get(info.family);
      if (!best || info.version[0] > best.version[0] || (info.version[0] === best.version[0] && info.version[1] > best.version[1])) {
        newest.set(info.family, info);
      }
    }
    const price = (info) => state.costs?.costs?.[info.spec] ?? Infinity;
    const main = [...newest.values()].sort((a, b) => price(a) - price(b));
    const older = mine.filter((i) => !main.includes(i)).sort((a, b) => price(a) - price(b) || a.label.localeCompare(b.label));
    const names = PROVIDERS[provider] || {family: provider, maker: provider};
    root.append(el("h2", {class: "group-title", text: t("ai.group", names)}));
    root.append(el("div", {class: "grid"}, main.map(aiCard)));
    if (older.length) {
      const open = state.showOlder || older.some((i) => state.selected.has(i.spec));
      root.append(el("button", {class: "link more-link", type: "button", "aria-expanded": String(open),
        text: open ? t("ai.hideOlder") : t("ai.older", {n: older.length}),
        onclick: () => { state.showOlder = !open; renderAis(); }}));
      if (open) root.append(el("div", {class: "grid"}, older.map(aiCard)));
    }
  }
  if (models.some((m) => !m.runnable)) root.append(el("p", {class: "muted small soon", text: t("ai.soon")}));
}

/* ---------- step 3: start ---------- */

function selectedPack() {
  return (state.catalog?.packs || []).find((p) => p.id === state.pack);
}

async function renderStart() {
  const pack = selectedPack();
  const specs = [...state.selected];
  const summary = specs.length === 1
    ? t("s.summaryOne", {ai: prettyName(specs[0]), n: pack?.tasks ?? 0, test: labelOf(pack || {name: state.pack}, "name")})
    : t("s.summaryMany", {count: specs.length, n: pack?.tasks ?? 0, test: labelOf(pack || {name: state.pack}, "name")});
  $("start-summary").textContent = summary;
  const repeats = $("repeats");
  if (!repeats.options.length || repeats.dataset.lang !== kmLang) {
    repeats.replaceChildren(...[1, 3, 5].map((n) => el("option", {value: String(n), text: n === 1 ? t("s.time1") : t("s.timeN", {n})})));
    repeats.dataset.lang = kmLang;
  }
  repeats.value = String(state.repeats);
  await refreshEstimate();
}

async function refreshEstimate() {
  const body = $("start-body");
  $("run").disabled = true;
  if (!body.childElementCount) body.replaceChildren(el("p", {class: "muted", text: t("s.checking")}));
  if (state.capInr === null) state.capInr = Math.round((state.catalog?.default_cap_usd ?? 1) * rate());
  try {
    state.estimate = await bridge.call("estimate", {
      suite: state.pack,
      runners: [...state.selected],
      repeats: state.repeats,
      max_usd: state.capInr / rate(),
    });
  } catch (error) {
    body.replaceChildren();
    showNotice(error.message);
    return;
  }
  const estimate = state.estimate;
  const paid = estimate.paid;
  body.replaceChildren();

  const rows = el("ul", {class: "list ai-rows"});
  const missing = new Set();
  for (const row of estimate.runners) {
    const provider = providerOf(row.spec);
    const needsKey = provider && !hasKey(provider);
    if (needsKey) missing.add(provider);
    const status = needsKey
      ? el("button", {class: "link", type: "button", text: `${t("ai.needsKey", {maker: PROVIDERS[provider]?.maker || provider})} · ${t("ai.connect")}`, onclick: () => openConnect(provider)})
      : el("span", {class: "muted small", text: row.ready ? t("s.ready") : (row.plain_problems[0] || "")});
    rows.append(el("li", {},
      el("div", {}, el("strong", {text: prettyName(row.spec, row.label)}), status),
      el("span", {class: "cost", text: row.worst_case_usd ? rupees(row.worst_case_usd) : t("ai.free")})));
  }
  body.append(rows);
  body.append(el("p", {class: "muted small", text: t("s.totalAnswers", {n: estimate.attempts})}));

  const money = el("div", {class: "money"});
  if (paid) {
    money.append(el("p", {class: "money-big", text: t("s.costMax", {money: rupees(estimate.worst_case_usd)})}),
      el("p", {class: "muted small", text: t("s.costNote", {rate: Math.round(rate())})}));
    const cap = el("input", {id: "cap", type: "number", min: "0", step: "10", inputmode: "decimal", value: String(state.capInr),
      onchange: (event) => { state.capInr = Math.max(0, Number(event.target.value) || 0); state.agreed = false; refreshEstimate(); }});
    money.append(el("label", {class: "field inline"}, el("span", {text: t("s.limit")}), cap),
      el("p", {class: "muted small", text: t("s.limitHelp")}));
  } else {
    money.append(el("p", {class: "money-big", text: t("s.costFree")}));
  }
  body.append(money);
  $("agree-wrap").hidden = !paid;
  $("agree").checked = state.agreed;
  $("agree-text").textContent = t("s.agree", {money: rupees(estimate.worst_case_usd)});

  const problems = estimate.ok ? [] : (estimate.plain_problems || []).filter((p) => !(missing.size && /API key/.test(p)));
  if (missing.size) {
    for (const provider of missing) problems.unshift(t("ai.needsKey", {maker: PROVIDERS[provider]?.maker || provider}) + ".");
  }
  if (problems.length) {
    body.append(el("div", {class: "problems"}, el("strong", {text: t("s.fix")}), el("ul", {}, [...new Set(problems)].map((p) => el("li", {text: p})))));
  }
  syncRun();
}

function syncRun() {
  const estimate = state.estimate;
  $("run").disabled = state.busy || !estimate?.ok || (estimate.paid && !state.agreed);
}

/* ---------- running ---------- */

function markFor(event) {
  if (event.passed === true) return {icon: ICON_RIGHT, text: t("r.right"), kind: "right"};
  if (event.passed === false) return {icon: ICON_WRONG, text: t("r.wrong"), kind: "wrong"};
  if (event.status === "skipped") return {icon: ICON_DASH, text: t("r.skipped"), kind: "other"};
  if (event.status === "error") return {icon: ICON_DASH, text: t("r.error"), kind: "other"};
  return {icon: ICON_DASH, text: t("r.pending"), kind: "other"};
}

function renderRunning() {
  const run = state.run;
  const fraction = run.total ? run.done / run.total : 0;
  $("meter-fill").style.width = (fraction * 100).toFixed(1) + "%";
  $("meter-read").textContent = t("r.count", {done: run.done, total: run.total});
  const tally = $("tally");
  tally.replaceChildren();
  for (const [spec, counts] of run.tallies) {
    tally.append(el("div", {class: "tally-row"},
      el("strong", {text: prettyName(spec)}),
      el("span", {}, svg(ICON_RIGHT, "icon mark right"), ` ${counts.right} ${t("r.right")}`),
      el("span", {}, svg(ICON_WRONG, "icon mark"), ` ${counts.wrong} ${t("r.wrong")}`),
      counts.other ? el("span", {class: "muted", text: `· ${counts.other}`}) : null));
  }
}

async function startRun() {
  if ($("run").disabled) return;
  state.busy = true;
  state.run = {done: 0, total: state.estimate?.attempts || 1, tallies: new Map(), stopping: false};
  for (const spec of state.selected) state.run.tallies.set(spec, {right: 0, wrong: 0, other: 0});
  $("feed").replaceChildren();
  $("now-asking").textContent = "";
  $("cancel").disabled = false;
  $("cancel").textContent = t("r.stop");
  go("running");
  renderRunning();
  try {
    const result = await bridge.call("run", {
      suite: state.pack,
      runners: [...state.selected],
      repeats: state.repeats,
      max_usd: state.capInr / rate(),
      confirmed: true,
      seed: 0,
    });
    state.busy = false;
    state.agreed = false;
    await showResults(result.run_id);
  } catch (error) {
    state.busy = false;
    $("now-asking").textContent = "";
    $("cancel").disabled = true;
    showNotice(t("r.failed") + " " + error.message);
    $("feed").before(el("div", {class: "actions"}, el("button", {class: "secondary", type: "button", text: t("c.back"), onclick: () => go("start")})));
  }
}

function onProgress(event) {
  const run = state.run;
  if (!run) return;
  if (event.type === "start") {
    run.total = event.total || run.total;
  } else if (event.type === "asking") {
    $("now-asking").textContent = t("r.asking", {ai: prettyName(event.runner, event.runner_label), q: event.title || event.task_id});
  } else if (event.type === "attempt") {
    run.done = event.index;
    const mark = markFor(event);
    const counts = run.tallies.get(event.runner) || {right: 0, wrong: 0, other: 0};
    counts[mark.kind === "right" ? "right" : mark.kind === "wrong" ? "wrong" : "other"] += 1;
    run.tallies.set(event.runner, counts);
    const feed = $("feed");
    feed.prepend(el("li", {class: "feed-item " + mark.kind},
      svg(mark.icon, "icon mark " + mark.kind),
      el("span", {class: "feed-text"}, el("strong", {text: prettyName(event.runner, event.runner_label)}), " · ", event.title || event.task_id),
      el("span", {class: "feed-mark", text: mark.text})));
    while (feed.childElementCount > 60) feed.lastElementChild.remove();
  } else if (event.type === "done") {
    run.done = run.total;
    $("now-asking").textContent = "";
  }
  renderRunning();
}

/* ---------- results ---------- */

function summarise(report) {
  const rows = report.results || [];
  const order = (report.models || []).map((m) => m.runner_id);
  return order.map((id) => {
    const mine = rows.filter((r) => r.runner_id === id);
    const graded = mine.filter((r) => r.grade_status === "graded");
    const right = graded.filter((r) => r.passed === true).length;
    const answered = mine.filter((r) => r.status === "ok");
    const time = answered.reduce((sum, r) => sum + (r.latency_s || 0), 0);
    const cost = mine.reduce((sum, r) => sum + (r.cost?.usd || 0), 0);
    return {
      id, label: prettyName(id, (report.models || []).find((m) => m.runner_id === id)?.configuration_name),
      total: mine.length, graded: graded.length, right,
      wrong: graded.length - right,
      errors: mine.filter((r) => r.status === "error").length,
      pending: mine.filter((r) => r.grade_status === "pending").length,
      skipped: mine.filter((r) => r.status === "skipped").length,
      time, avg: answered.length ? time / answered.length : 0, cost, cells: mine,
    };
  });
}

async function showResults(runId) {
  state.runId = runId;
  state.report = null;
  go("results");
  selectTab("card");
  $("panel-card").replaceChildren(el("p", {class: "muted", text: t("res.loading")}));
  $("panel-answers").replaceChildren();
  $("dashboard").removeAttribute("src");
  try {
    state.report = await bridge.call("report", {run_id: runId});
  } catch (error) {
    $("panel-card").replaceChildren();
    showNotice(error.message);
    return;
  }
  renderReportCard();
}

function renderReportCard() {
  const report = state.report;
  if (!report) return;
  const packs = state.catalog?.packs || [];
  const pack = packs.find((p) => p.id === report.benchmark?.name);
  $("results-test").textContent = pack ? labelOf(pack, "name") : (report.benchmark?.name || "");
  $("tab-answers").hidden = !pack?.mine;
  const panel = $("panel-card");
  panel.replaceChildren();
  if (report.run?.status && report.run.status !== "completed") panel.append(el("p", {class: "notice", text: t("res.stopped")}));
  const people = summarise(report);
  const cards = el("div", {class: "report"});
  for (const person of people) {
    const percent = person.graded ? Math.round((person.right / person.graded) * 100) : 0;
    const strip = el("div", {class: "strip", role: "img", "aria-label": t("res.of", {right: person.right, total: person.total})});
    if (person.total <= 60) {
      for (const cell of person.cells) {
        const kind = cell.passed === true ? "right" : cell.passed === false ? "wrong" : "other";
        strip.append(el("span", {class: "cell " + kind}));
      }
    } else {
      strip.classList.add("bar");
      strip.append(el("span", {class: "bar-fill", style: `width:${percent}%`}));
    }
    const notes = [];
    if (person.errors) notes.push(t("res.errors", {n: person.errors}));
    if (person.pending) notes.push(t("res.pending", {n: person.pending}));
    cards.append(el("article", {class: "card report-row"},
      el("div", {class: "report-head"},
        el("h2", {text: person.label}),
        el("p", {class: "score"}, el("strong", {text: `${person.right}/${person.total}`}), el("span", {class: "muted", text: t("res.of", {right: person.right, total: person.total})}))),
      strip,
      el("p", {class: "muted small", text: [
        person.time >= 1 ? t("res.time", {t: duration(person.time), avg: duration(person.avg)}) : t("res.instant"),
        person.cost > 0 ? t("res.cost", {money: rupees(person.cost)}) : t("ai.free"),
        ...notes,
      ].filter(Boolean).join(" · ")})));
  }
  panel.append(cards);

  const verdicts = (report.verdicts || []).filter((v) => v.metric === "accuracy");
  if (verdicts.length) {
    const list = el("ul", {class: "verdicts"});
    for (const verdict of verdicts) {
      const a = prettyName(verdict.runner_a);
      const b = prettyName(verdict.runner_b);
      let text;
      if (verdict.a_better === 0 && verdict.b_better === 0) text = t("res.same", {a, b});
      else if (verdict.verdict === "clearly_different") {
        const winnerIsA = verdict.favours === verdict.runner_a;
        text = t("res.clear", {a: winnerIsA ? a : b, b: winnerIsA ? b : a});
      } else text = t("res.close", {a, b});
      list.append(el("li", {text}));
    }
    panel.append(el("h2", {class: "group-title", text: t("res.compare")}), list);
  }
  panel.append(el("p", {class: "honest", text: t("res.honest")}));
}

function selectTab(name) {
  for (const tab of ["card", "answers", "details"]) {
    $("tab-" + tab).setAttribute("aria-selected", String(tab === name));
    $("panel-" + tab).hidden = tab !== name;
  }
  if (name === "answers") renderAnswers();
  if (name === "details") loadDetails();
}

async function renderAnswers() {
  const panel = $("panel-answers");
  if (panel.dataset.run === state.runId && panel.childElementCount) return;
  panel.replaceChildren(el("p", {class: "muted", text: t("c.loading")}));
  let data;
  try {
    data = await bridge.call("answers", {run_id: state.runId});
  } catch (error) {
    panel.replaceChildren(el("p", {class: "muted", text: /yourself/.test(error.message) ? t("res.onlyMine") : error.message}));
    return;
  }
  panel.dataset.run = state.runId;
  panel.replaceChildren();
  const checkName = {includes: "b.includes", number: "b.number", exact: "b.exact"};
  const repeats = new Set(data.answers.map((a) => a.repeat)).size > 1;
  data.questions.forEach((question, index) => {
    const replies = data.answers.filter((a) => a.task_id === question.id);
    if (!replies.length) return;
    const list = el("ul", {class: "replies"});
    for (const reply of replies) {
      const mark = markFor(reply);
      const text = reply.output ? reply.output : (reply.error || t("res.noReply"));
      list.append(el("li", {class: mark.kind},
        el("div", {class: "reply-head"},
          svg(mark.icon, "icon mark " + mark.kind),
          el("strong", {text: prettyName(reply.runner, reply.label)}),
          repeats ? el("span", {class: "muted small", text: t("res.try", {n: (reply.repeat || 0) + 1})}) : null,
          el("span", {class: "feed-mark", text: mark.text})),
        el("p", {class: "reply-text" + (reply.output ? "" : " muted"), text})));
    }
    panel.append(el("article", {class: "card answer-card"},
      el("h2", {}, el("span", {class: "muted", text: `${index + 1}. `}), question.question),
      el("p", {class: "small"}, el("strong", {text: t("res.rightAnswer") + ": "}), question.answer, el("span", {class: "muted", text: ` · ${t(checkName[question.check] || "b.includes")}`})),
      replies.some((r) => r.edited) ? el("p", {class: "muted small", text: t("res.edited")}) : null,
      list));
  });
}

async function loadDetails() {
  const frame = $("dashboard");
  if (frame.getAttribute("src")) return;
  try {
    const {url} = await bridge.call("dashboard_url", {run_id: state.runId, theme: window.kmTheme ? window.kmTheme() : "system"});
    frame.src = url;
  } catch (error) {
    showNotice(error.message);
  }
}

async function saveReport() {
  try {
    const card = await bridge.call("share_card", {run_id: state.runId});
    try { await navigator.clipboard.writeText(card.summary); } catch { /* The file is still saved. */ }
    const where = await bridge.saveText("krutrim-mulyankan-report.html", card.html);
    if (where) showNotice(t("res.saved"));
  } catch (error) {
    showNotice(error.message);
  }
}

function runAgain() {
  const report = state.report;
  if (report) {
    const known = new Set((state.catalog?.packs || []).map((p) => p.id));
    if (known.has(report.benchmark?.name)) state.pack = report.benchmark.name;
    const runners = (report.models || []).map((m) => m.runner_id);
    if (runners.length) state.selected = new Set(runners);
    savePicks();
  }
  go("start");
}

/* ---------- AI accounts ---------- */

function providerCard(provider, onSaved) {
  const saved = hasKey(provider.id);
  const status = el("p", {class: "saved small", role: "status"});
  const input = el("input", {type: "password", autocomplete: "off", spellcheck: false, "aria-label": `${provider.name}: ${t("acc.paste")}`, placeholder: t("acc.paste")});
  const save = el("button", {class: "secondary", type: "button", text: t("acc.save")});
  const submit = async (secret) => {
    showNotice("");
    try {
      const where = await bridge.saveKey(provider.id, secret);
      input.value = "";
      await refreshCatalog();
      const nowSaved = hasKey(provider.id);
      status.textContent = !secret ? t("acc.removed") : nowSaved ? (where === "keychain" ? t("acc.savedKeychain") : t("acc.savedSession")) : "";
      if (onSaved) onSaved();
    } catch (error) {
      showNotice(error.message);
    }
  };
  save.addEventListener("click", () => {
    const secret = input.value.trim();
    if (!secret) { status.textContent = t("acc.empty"); input.focus(); return; }
    submit(secret);
  });
  input.addEventListener("keydown", (event) => { if (event.key === "Enter") save.click(); });
  const steps = el("ol", {class: "howto small"},
    el("li", {}, provider.signup
      ? el("a", {href: provider.signup, target: "_blank", rel: "noopener", text: t("acc.open") + " ↗",
        onclick: (event) => { event.preventDefault(); bridge.openExternal(provider.signup); }})
      : null, " ", t("acc.step1")),
    el("li", {text: t("acc.step2")}),
    el("li", {text: t("acc.step3")}));
  return el("article", {class: "card provider" + (saved ? " selected" : "")},
    el("div", {class: "provider-head"},
      el("h2", {text: provider.name}),
      el("span", {class: "pill" + (saved ? " on" : ""), text: saved ? "✓ " + t("acc.connected") : t("acc.notConnected")})),
    el("p", {class: "muted small", text: t("maker." + provider.id) || labelOf(provider, "blurb")}),
    saved ? null : steps,
    el("div", {class: "key-row"}, input, save),
    status,
    saved ? el("button", {class: "link", type: "button", text: t("acc.remove"), onclick: () => submit("")}) : null);
}

function usefulProviders() {
  const runnable = new Set((state.catalog?.models || []).filter((m) => m.runnable && !m.free).map((m) => m.provider));
  const all = (state.catalog?.providers || []).filter((p) => PROVIDERS[p.id]);
  return {main: all.filter((p) => runnable.has(p.id)), other: all.filter((p) => !runnable.has(p.id))};
}

function renderAccounts() {
  const {main, other} = usefulProviders();
  $("provider-list").replaceChildren(...main.map((p) => providerCard(p, renderAccounts)));
  $("provider-list-other").replaceChildren(...other.map((p) => providerCard(p, renderAccounts)));
  $("other-providers").hidden = !other.length;
}

function openConnect(providerId) {
  const provider = (state.catalog?.providers || []).find((p) => p.id === providerId);
  if (!provider) { go("accounts"); return; }
  state.modalProvider = providerId;
  const render = () => {
    $("modal-body").replaceChildren(providerCard(provider, render));
    if (hasKey(providerId)) $("modal-done").focus();
  };
  render();
  $("modal").hidden = false;
  $("modal-body").querySelector("input")?.focus();
}

function closeConnect() {
  if ($("modal").hidden) return;
  $("modal").hidden = true;
  state.modalProvider = null;
  if (state.screen === "ais") renderAis();
  if (state.screen === "start") refreshEstimate();
}

async function refreshCatalog() {
  state.catalog = await bridge.call("catalog");
}

/* ---------- wiring ---------- */

function themeLabel() {
  const preference = window.kmThemePreference ? window.kmThemePreference() : "system";
  $("theme-cycle").textContent = t("theme." + preference);
}

document.addEventListener("DOMContentLoaded", async () => {
  applyI18n();
  themeLabel();
  document.addEventListener("km-theme", themeLabel);
  for (const button of document.querySelectorAll("[data-lang]")) {
    button.addEventListener("click", () => setLang(button.dataset.lang));
  }
  document.addEventListener("km-lang", () => {
    themeLabel();
    if (state.screen === "running") renderRunning();
    else if (state.screen === "results") { renderReportCard(); $("panel-answers").replaceChildren(); if (!$("panel-answers").hidden) renderAnswers(); }
    else if (state.screen === "builder") renderBuilder();
    else go(state.screen);
    if (!$("modal").hidden && state.modalProvider) openConnect(state.modalProvider);
  });
  $("theme-cycle").addEventListener("click", () => window.kmCycleTheme && window.kmCycleTheme());
  $("home-link").addEventListener("click", (event) => { event.preventDefault(); if (!state.busy) go("home"); });
  $("accounts-link").addEventListener("click", () => { if (!state.busy) { state.returnTo = state.screen; go("accounts"); } });
  $("accounts-done").addEventListener("click", () => go(state.returnTo && state.returnTo !== "accounts" ? state.returnTo : "home"));
  $("home-start").addEventListener("click", () => go("questions"));
  $("home-write").addEventListener("click", () => newTest());
  for (const button of document.querySelectorAll("[data-goto]")) {
    button.addEventListener("click", () => go(button.dataset.goto));
  }
  $("questions-next").addEventListener("click", () => {
    if (!selectedPack()) { showNotice(t("q.pick")); return; }
    go("ais");
  });
  $("add-question").addEventListener("click", () => {
    state.draft.questions.push(blankQuestion());
    renderEditor();
    const items = $("question-editor").querySelectorAll("textarea");
    items[items.length - 1]?.focus();
  });
  $("builder-cancel").addEventListener("click", () => { state.draft = null; go("questions"); });
  $("builder-save").addEventListener("click", saveDraft);
  $("ais-next").addEventListener("click", () => {
    if (!state.selected.size) { showNotice(t("ai.none")); return; }
    state.agreed = false;
    go("start");
  });
  $("repeats").addEventListener("change", (event) => {
    state.repeats = Number(event.target.value) || 1;
    state.agreed = false;
    state.costsFor = "";
    refreshEstimate();
  });
  $("run").addEventListener("click", startRun);
  $("agree").addEventListener("change", (event) => { state.agreed = event.target.checked; syncRun(); });
  $("cancel").addEventListener("click", () => {
    if (state.run) state.run.stopping = true;
    $("cancel").disabled = true;
    $("cancel").textContent = t("r.stopping");
    bridge.call("cancel").catch((error) => showNotice(error.message));
  });
  for (const tab of ["card", "answers", "details"]) $("tab-" + tab).addEventListener("click", () => selectTab(tab));
  $("again").addEventListener("click", runAgain);
  $("save-report").addEventListener("click", saveReport);
  $("new-test").addEventListener("click", () => go("questions"));
  $("modal-done").addEventListener("click", closeConnect);
  $("modal").addEventListener("click", (event) => { if (event.target === $("modal")) closeConnect(); });
  document.addEventListener("keydown", (event) => { if (event.key === "Escape") closeConnect(); });
  bridge.onProgress(onProgress);

  loadPicks();
  try {
    await bridge.init();
    let lastError = null;
    for (let attempt = 0; attempt < 20; attempt += 1) {
      try {
        await refreshCatalog();
        lastError = null;
        break;
      } catch (error) {
        lastError = error;
        await new Promise((resolve) => setTimeout(resolve, 250));
      }
    }
    if (lastError) throw lastError;
    if (!state.catalog.packs.some((pack) => pack.id === state.pack) && state.catalog.packs[0]) {
      state.pack = state.catalog.default_pack || state.catalog.packs[0].id;
    }
    go("home");
  } catch (error) {
    showNotice(error.message);
  }
});
