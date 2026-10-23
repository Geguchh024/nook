// Nook plugin helper for Node.js 18+. No dependencies: copy this file next
// to your plugin's main.js.
//
//   const nook = require("./nook");
//   const app = new nook.Plugin();
//   let count = 0;
//   const draw = () => app.tile(nook.column([
//     nook.text(String(count), { size: "big" }),
//     nook.row([nook.button("dec", { icon: "minus" }), nook.button("inc", { icon: "plus" })]),
//   ], { align: "center", justify: "center" }));
//   app.on("inc", () => { count++; draw(); });
//   app.on("dec", () => { count--; draw(); });
//   draw();
//
// Protocol reference: docs/plugins.html in the Nook repository.
"use strict";

const fs = require("fs");
const path = require("path");
const readline = require("readline");

const clean = (o) => Object.fromEntries(Object.entries(o).filter(([, v]) => v !== undefined && v !== null));
const node = (type, fields = {}) => clean({ type, ...fields });

// Layout
const column = (children, o = {}) => node("column", { children, ...o }); // gap, align, justify, scroll
const row = (children, o = {}) => node("row", { children, ...o }); // gap, align
const grid = (columns, children, o = {}) => node("grid", { columns, children, ...o });
const card = (children, o = {}) => node("card", { children, ...o }); // padding, gap
const spacer = (size) => node("spacer", { size });
const divider = () => node("divider");

// Content
const text = (value, o = {}) => node("text", { text: String(value), ...o }); // size, color, bold, wrap, mono
const icon = (name, o = {}) => node("icon", { icon: name, ...o }); // size, color, fill
const image = (o) => node("image", o); // path | base64, width, height, rounding
const progress = (value, o = {}) => node("progress", { value, ...o });
const ring = (value, o = {}) => node("ring", { value, ...o }); // label, size, color
const sparkline = (values, o = {}) => node("sparkline", { values, ...o }); // max, color, width, height
const item = (id, title, o = {}) => clean({ id, title, ...o }); // subtitle, icon, trailing, selected
const listView = (items, o = {}) => node("list", { items, ...o });
const canvas = (height, shapes, o = {}) => node("canvas", { height, shapes, ...o }); // width, id

// Controls
const button = (id, o = {}) => node("button", { id, ...o }); // label, icon, primary, width
const toggle = (id, value, o = {}) => node("toggle", { id, value: !!value, ...o }); // label
const slider = (id, value, o = {}) => node("slider", { id, value, ...o }); // min, max, step, width
const textInput = (id, value = "", o = {}) => node("input", { id, value, ...o }); // hint, width, live
const chips = (id, options, selected = 0, o = {}) => node("chips", { id, options, selected, ...o });

// Canvas shapes
const shape = {
  rect: (x, y, w, h, o = {}) => clean({ type: "rect", x, y, w, h, ...o }), // fill, stroke, width, radius
  circle: (x, y, r, o = {}) => clean({ type: "circle", x, y, r, ...o }), // fill, stroke, width
  line: (points, o = {}) => clean({ type: "line", points, ...o }), // color, width, closed
  polygon: (points, o = {}) => clean({ type: "polygon", points, ...o }), // fill
  arc: (x, y, r, start, end, o = {}) => clean({ type: "arc", x, y, r, start, end, ...o }), // color, width
  text: (x, y, value, o = {}) => clean({ type: "text", x, y, text: String(value), ...o }), // size, color, anchor, bold
  icon: (x, y, name, o = {}) => clean({ type: "icon", x, y, icon: name, ...o }), // size, color, fill
};

function emit(message) {
  process.stdout.write(JSON.stringify(message) + "\n");
}

/** In poll mode: the event that caused this run, or null. */
function pollEvent() {
  const raw = process.env.NOOK_EVENT;
  return raw ? JSON.parse(raw) : null;
}

class Plugin {
  constructor() {
    this.id = process.env.NOOK_PLUGIN_ID || "dev";
    this.dataDir = process.env.NOOK_DATA_DIR || path.join(process.cwd(), ".data");
    fs.mkdirSync(this.dataDir, { recursive: true });
    this.dark = (process.env.NOOK_THEME || "dark") === "dark";
    this.accent = process.env.NOOK_ACCENT || "#7C5CFF";
    this.tile = { size: null, width: null, height: null };
    this.pageOpen = false;
    this.handlers = new Map();
    this.listeners = new Map();
    const rl = readline.createInterface({ input: process.stdin });
    rl.on("line", (line) => {
      if (!line.trim()) return;
      let msg;
      try { msg = JSON.parse(line); } catch { return; }
      try { this.dispatch(msg); } catch (e) { this.log(String(e && e.stack || e), "error"); }
    });
    rl.on("close", () => process.exit(0));
  }

  /** handler(event) for a control's events ("click", "change", "submit", "commit"). */
  on(eventId, fn) { this.handlers.set(eventId, fn); return this; }

  /** Nook's own messages: hello, theme, layout, tile_click, page_open, page_close, hotkey, files, shutdown. */
  onMessage(type, fn) {
    if (!this.listeners.has(type)) this.listeners.set(type, []);
    this.listeners.get(type).push(fn);
    return this;
  }

  dispatch(msg) {
    const t = msg.type;
    if (t === "hello") Object.assign(this, { dark: msg.theme.dark, accent: msg.theme.accent });
    if (t === "theme") Object.assign(this, { dark: msg.dark, accent: msg.accent });
    if (t === "layout" && msg.target === "tile") this.tile = { size: msg.size, width: msg.width, height: msg.height };
    if (t === "page_open") this.pageOpen = true;
    if (t === "page_close") this.pageOpen = false;
    if (t === "event" && this.handlers.has(msg.id)) this.handlers.get(msg.id)(msg);
    for (const fn of this.listeners.get(t) || []) fn(msg);
    if (t === "shutdown") process.exit(0);
  }

  setTile(view) { emit({ type: "tile", view }); }
  setPage(view, actions) { emit(clean({ type: "page", view, actions })); }
  badgeRing(progress, label = "") { emit({ type: "badge", badge: { kind: "ring", progress, label } }); }
  badgeGlyph(name) { emit({ type: "badge", badge: { kind: "glyph", icon: name } }); }
  badgePulse() { emit({ type: "badge", badge: { kind: "pulse" } }); }
  clearBadge() { emit({ type: "badge", badge: null }); }
  active(on = true) { emit({ type: "active", active: !!on }); }
  openPage() { emit({ type: "open_page" }); }
  closePage() { emit({ type: "close_page" }); }
  show() { emit({ type: "show" }); }
  copy(value) { emit({ type: "copy", text: String(value) }); }
  open(target) { emit({ type: "open", target }); }
  log(message, level = "info") { emit({ type: "log", level, message: String(message) }); }

  load(name, fallback = null) {
    try { return JSON.parse(fs.readFileSync(path.join(this.dataDir, name + ".json"), "utf8")); }
    catch { return fallback; }
  }
  save(name, value) {
    const file = path.join(this.dataDir, name + ".json");
    fs.writeFileSync(file + ".tmp", JSON.stringify(value, null, 1));
    fs.renameSync(file + ".tmp", file);
  }
}

module.exports = {
  Plugin, emit, pollEvent, shape,
  column, row, grid, card, spacer, divider,
  text, icon, image, progress, ring, sparkline, item, listView, canvas,
  button, toggle, slider, textInput, chips,
};
