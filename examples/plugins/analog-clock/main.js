const nook = require("./nook");
const { shape } = nook;

const app = new nook.Plugin();
let style = app.load("style", 0); // 0 minimal, 1 numbers, 2 neon
const cities = [
  ["London", "Europe/London"],
  ["New York", "America/New_York"],
  ["Tokyo", "Asia/Tokyo"],
  ["Sydney", "Australia/Sydney"],
];

function timeIn(zone) {
  const parts = new Intl.DateTimeFormat("en-GB", {
    timeZone: zone, hour: "numeric", minute: "numeric", second: "numeric", hour12: false,
  }).formatToParts(new Date());
  const get = (t) => Number(parts.find((p) => p.type === t).value);
  return { h: get("hour") % 24, m: get("minute"), s: get("second") };
}

// One clock face as canvas shapes, centered in a size x size box.
function face(size, t, withSeconds = true) {
  const c = size / 2, r = size / 2 - 4;
  const hand = (deg, len, width, color) => {
    const a = ((deg - 90) * Math.PI) / 180;
    return shape.line([[c, c], [c + Math.cos(a) * len, c + Math.sin(a) * len]], { color, width });
  };
  const neon = style === 2;
  const shapes = [shape.circle(c, c, r, { fill: neon ? "#0B0B1A" : undefined, stroke: neon ? "accent" : "faint", width: neon ? 2 : 1 })];
  for (let i = 0; i < 12; i++) {
    const a = ((i * 30 - 90) * Math.PI) / 180;
    if (style === 1) {
      shapes.push(shape.text(c + Math.cos(a) * (r - 11), c + Math.sin(a) * (r - 11), i === 0 ? 12 : i, { size: size / 14, anchor: "center", color: "dim" }));
    } else {
      const inner = i % 3 === 0 ? r - 9 : r - 5;
      shapes.push(shape.line([[c + Math.cos(a) * inner, c + Math.sin(a) * inner], [c + Math.cos(a) * (r - 2), c + Math.sin(a) * (r - 2)]], { color: neon ? "accent" : "dim", width: i % 3 === 0 ? 2 : 1 }));
    }
  }
  shapes.push(hand((t.h % 12) * 30 + t.m / 2, r * 0.5, 3.5, neon ? "#FFFFFF" : "text"));
  shapes.push(hand(t.m * 6 + t.s / 10, r * 0.75, 2.5, neon ? "#FFFFFF" : "text"));
  if (withSeconds) shapes.push(hand(t.s * 6, r * 0.85, 1, "accent"));
  shapes.push(shape.circle(c, c, 3, { fill: "accent" }));
  return shapes;
}

function draw() {
  const now = new Date();
  const local = { h: now.getHours(), m: now.getMinutes(), s: now.getSeconds() };
  const size = Math.min(app.tile.width || 132, app.tile.height || 132);
  app.setTile(nook.column([
    nook.canvas(size, face(size, local), { width: size, id: "face", tooltip: "Click to change style" }),
  ], { align: "center", justify: "center" }));

  if (app.pageOpen) {
    app.setPage(nook.grid(4, cities.map(([name, zone]) => {
      const t = timeIn(zone);
      return nook.column([
        nook.canvas(110, face(110, t, false), { width: 110 }),
        nook.text(name, { bold: true }),
        nook.text(`${String(t.h).padStart(2, "0")}:${String(t.m).padStart(2, "0")}`, { color: "dim", mono: true }),
      ], { align: "center", gap: 4 });
    })));
  } else {
    // A page exists, so a tile click opens it; it is filled when shown.
    app.setPage(nook.text("…"));
  }
}

app.on("face", () => {
  style = (style + 1) % 3;
  app.save("style", style);
  draw();
});
app.onMessage("layout", draw);
app.onMessage("theme", draw);
app.onMessage("page_open", draw);

draw();
// Tick on the second boundary.
setTimeout(() => { draw(); setInterval(draw, 1000); }, 1000 - (Date.now() % 1000));
