const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const {test} = require("node:test");

class Element {
  constructor(tag = "div", props = {}, ...children) {
    this.tag = tag;
    this.children = [];
    this.style = {};
    this.attributes = {};
    this.listeners = {};
    this.className = props.class || "";
    this.textContent = props.text || "";
    this.hidden = false;
    this.append(...children.flat());
  }
  append(...children) { this.children.push(...children); }
  prepend(...children) { this.children.unshift(...children); }
  replaceChildren(...children) { this.textContent = ""; this.children = children; }
  setAttribute(key, value) { this.attributes[key] = value; }
  addEventListener(name, callback) { this.listeners[name] = callback; }
  querySelectorAll(selector) {
    return this.children.flatMap(child => {
      if (!(child instanceof Element)) return [];
      const matches = child.className.split(" ").includes(selector.slice(1));
      return [...(matches ? [child] : []), ...child.querySelectorAll(selector)];
    });
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0]; }
  get text() { return this.textContent + this.children.map(child => child instanceof Element ? child.text : child).join(""); }
}

function setup(lang = "en") {
  const nodes = new Map();
  const listeners = new Map();
  const $ = id => {
    if (!nodes.has(id)) nodes.set(id, new Element());
    return nodes.get(id);
  };
  let onProgress;
  let savedText;
  const context = vm.createContext({
    window: {matchMedia: () => ({matches: true}), addEventListener() {}},
    document: {
      addEventListener: (name, callback) => {
        if (!listeners.has(name)) listeners.set(name, []);
        listeners.get(name).push(callback);
      },
      querySelectorAll: () => [],
      createTextNode: text => text,
    },
    localStorage: {getItem: () => lang},
    bridge: {
      onProgress: callback => { onProgress = callback; },
      saveText: async (_name, text) => { savedText = text; return "comparison.md"; },
    },
    state: {screen: "side"},
    performance: {now: () => 5000},
    $,
    el: (tag, props, ...children) => new Element(tag, props, ...children),
    svg: () => new Element("svg"),
    splitModelName: model => ({short: model.name, maker: "Mock"}),
    rate: () => 88,
    rupees: usd => `₹${(usd * 88).toFixed(2)}`,
  });
  const ui = path.join(__dirname, "../ui");
  vm.runInContext(fs.readFileSync(path.join(ui, "i18n.js"), "utf8"), context);
  vm.runInContext(fs.readFileSync(path.join(ui, "side.js"), "utf8"), context);
  for (const listener of listeners.get("DOMContentLoaded")) listener();
  const side = vm.runInContext("sideBySide", context);
  function pane(index) {
    const stats = new Element("div", {}, ...Array.from({length: 4}, () => new Element("span", {class: "stat-value"})));
    return {
      id: `model-${index}`, m: {id: `model-${index}`, name: `Model ${index}`},
      node: new Element("article", {}, new Element("span", {class: "pane-badges"})),
      state: new Element(), stats,
      track: new Element("div", {}, ...Array.from({length: 4}, () => new Element())),
      flag: new Element(), answer: new Element(), body: new Element(), metricsBox: new Element(),
      thinkingBox: new Element(), firstThinking: null, firstAnswer: 0.1, connected: 0.05,
      text: "Partial answer", thinking: "", view: "answer", follow: false, done: false,
    };
  }
  function finish(costs, overrides = []) {
    const panes = costs.map((_, index) => pane(index));
    side.state.ask = {id: "ask-test", panes, started: 0};
    costs.forEach((cost, slot) => onProgress({kind: "live", ask_id: "ask-test", slot, type: "done", metrics: {
      cost_usd: cost, total_s: 1 + slot, first_answer_s: 0.1, stopped: false, ...overrides[slot],
    }}));
    return panes;
  }
  return {finish, nodes, savedText: () => savedText,
    progress: event => onProgress({kind: "live", ask_id: "ask-test", ...event})};
}

test("an incomplete answer keeps its text and receives no completion badges", () => {
  const ui = setup();
  const panes = ui.finish([0.01, 0.02, 0.03], [{incomplete: true}, {}, {}]);
  assert.equal(panes[0].state.attributes["aria-label"], "Incomplete");
  assert.match(panes[0].answer.text, /Partial answer/);
  assert.match(panes[0].answer.text, /connection ended/i);
  assert.equal(panes[0].node.querySelector(".pane-badges").children.length, 0);
  assert.equal(panes[0].flag.hidden, true);
  assert.equal(panes[1].state.attributes["aria-label"], "Done");
  assert.ok(panes[1].node.querySelector(".pane-badges").children.length > 0);
});

test("the incomplete state and explanation are available in Hindi", () => {
  const ui = setup("hi");
  const panes = ui.finish([null], [{incomplete: true}]);
  assert.equal(panes[0].state.attributes["aria-label"], "अधूरा");
  assert.match(panes[0].answer.text, /कनेक्शन बंद हो गया/);
  assert.match(panes[0].answer.text, /Partial answer/);
});

for (const lang of ["en", "hi"]) {
  test(`${lang}: unknown, mixed, zero and known costs stay distinct`, () => {
    const ui = setup(lang);
    ui.finish([null, undefined]);
    assert.equal(ui.nodes.get("side-summary").querySelector(".sum-total").text, lang === "en" ? "Cost unavailable" : "खर्च की जानकारी नहीं");
    ui.finish([0.01, null]);
    assert.match(ui.nodes.get("side-summary").querySelector(".sum-total").text, lang === "en" ? /Known cost: ₹0.88 · 1 unreported/ : /ज्ञात खर्च: ₹0.88 · 1 की जानकारी नहीं/);
    ui.finish([0, 0]);
    assert.equal(ui.nodes.get("side-summary").querySelector(".sum-total").text, "₹0.00");
    ui.finish([0.01, 0.02]);
    assert.match(ui.nodes.get("side-summary").querySelector(".sum-total").text, /₹2.64$/);
  });
}

test("late server costs refresh panes and summary, including a reported zero", () => {
  const ui = setup();
  const panes = ui.finish([null, 0.01], [{incomplete: true}, {}]);
  ui.progress({slot: 0, type: "stats", stats: {total_cost_usd: 0.02}});
  assert.match(ui.nodes.get("side-summary").querySelector(".sum-total").text, /₹2.64$/);
  assert.equal(panes[0].stats.querySelectorAll(".stat-value")[3].textContent, "1.76");
  assert.equal(panes[0].state.attributes["aria-label"], "Incomplete");
  assert.equal(panes[0].node.querySelector(".pane-badges").children.length, 0);
  const zero = ui.finish([null]);
  ui.progress({slot: 0, type: "stats", stats: {total_cost_usd: 0}});
  assert.equal(ui.nodes.get("side-summary").querySelector(".sum-total").text, "₹0.00");
  assert.equal(zero[0].stats.querySelectorAll(".stat-value")[3].textContent, "Free");
});

test("invalid late prices stay unknown and do not overwrite a reported stream cost", () => {
  const ui = setup();
  ui.finish([null]);
  for (const cost of [-1, NaN, Infinity, "0.02"]) {
    ui.progress({slot: 0, type: "stats", stats: {total_cost_usd: cost}});
    assert.equal(ui.nodes.get("side-summary").querySelector(".sum-total").text, "Cost unavailable");
  }
  ui.finish([0]);
  ui.progress({slot: 0, type: "stats", stats: {total_cost_usd: 0.02}});
  assert.equal(ui.nodes.get("side-summary").querySelector(".sum-total").text, "₹0.00");
});

test("a cheapest badge waits until every eligible answer has a reported cost", () => {
  const ui = setup();
  const panes = ui.finish([0.01, 0.02, null]);
  const cheapest = () => panes.flatMap(pane => pane.node.querySelector(".pane-badges").children)
    .filter(badge => badge.attributes["aria-label"] === "Cheapest");
  assert.equal(cheapest().length, 0);
  ui.progress({slot: 2, type: "stats", stats: {total_cost_usd: 0.03}});
  assert.equal(cheapest().length, 1);
  assert.ok(panes[0].node.querySelector(".pane-badges").children.some(badge => badge.attributes["aria-label"] === "Cheapest"));
});

test("saved comparisons retain incomplete text and later reported cost", async () => {
  const ui = setup();
  ui.finish([null], [{incomplete: true}]);
  ui.progress({slot: 0, type: "stats", stats: {total_cost_usd: 0.02}});
  await ui.nodes.get("side-save").listeners.click();
  assert.match(ui.savedText(), /Partial answer/);
  assert.match(ui.savedText(), /answer is incomplete/);
  assert.match(ui.savedText(), /₹1.76/);
});
