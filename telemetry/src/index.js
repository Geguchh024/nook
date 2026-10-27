// Nook usage stats: installs and daily opens, counted anonymously.
//
//   POST /ping   {"id": "<random install id>", "version": "0.2.0", "os": "10.0.26100", "first": true}
//   GET  /stats?key=<STATS_KEY>               dashboard
//   GET  /stats?key=<STATS_KEY>&format=json   the same numbers as JSON
//
// Nook sends at most one ping per day per install. Nothing identifying is
// kept: the id is random, and the request's IP is never read or stored.

const ID = /^[0-9a-f]{32}$/;
const VERSION = /^[0-9A-Za-z.+-]{1,24}$/;
const OS = /^[0-9.]{0,24}$/;

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/ping" && request.method === "POST") {
      return ping(request, env);
    }
    if (url.pathname === "/stats" && request.method === "GET") {
      if (!env.STATS_KEY || url.searchParams.get("key") !== env.STATS_KEY) {
        return new Response("Not found", { status: 404 });
      }
      const stats = await collect(env);
      if (url.searchParams.get("format") === "json") {
        return Response.json(stats);
      }
      return new Response(page(stats), {
        headers: { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" },
      });
    }
    return new Response("Not found", { status: 404 });
  },
};

async function ping(request, env) {
  let body;
  try {
    body = await request.json();
  } catch {
    return new Response("bad json", { status: 400 });
  }
  const id = String(body.id ?? "").toLowerCase();
  const version = String(body.version ?? "");
  const os = String(body.os ?? "");
  if (!ID.test(id) || !VERSION.test(version) || !OS.test(os)) {
    return new Response("bad ping", { status: 400 });
  }
  const day = new Date().toISOString().slice(0, 10);
  await env.DB.prepare(
    `INSERT INTO pings (id, day, version, os, first) VALUES (?1, ?2, ?3, ?4, ?5)
     ON CONFLICT (id, day) DO UPDATE SET version = excluded.version,
       first = MAX(pings.first, excluded.first)`,
  )
    .bind(id, day, version, os, body.first ? 1 : 0)
    .run();
  return new Response(null, { status: 204 });
}

async function collect(env) {
  const one = async (sql, ...args) => (await env.DB.prepare(sql).bind(...args).first()) ?? {};
  const all = async (sql, ...args) => (await env.DB.prepare(sql).bind(...args).all()).results;
  const daysAgo = (n) => new Date(Date.now() - n * 864e5).toISOString().slice(0, 10);

  const [installs, today, week, month, daily, versions] = await Promise.all([
    one("SELECT COUNT(DISTINCT id) AS n FROM pings"),
    one("SELECT COUNT(*) AS n FROM pings WHERE day = ?1", daysAgo(0)),
    one("SELECT COUNT(DISTINCT id) AS n FROM pings WHERE day > ?1", daysAgo(7)),
    one("SELECT COUNT(DISTINCT id) AS n FROM pings WHERE day > ?1", daysAgo(30)),
    all(
      `SELECT day, COUNT(*) AS active, SUM(first) AS new FROM pings
       WHERE day > ?1 GROUP BY day ORDER BY day`,
      daysAgo(30),
    ),
    all(
      `SELECT version, COUNT(DISTINCT id) AS n FROM pings
       WHERE day > ?1 GROUP BY version ORDER BY n DESC`,
      daysAgo(7),
    ),
  ]);
  return {
    installs: installs.n ?? 0,
    active_today: today.n ?? 0,
    active_7d: week.n ?? 0,
    active_30d: month.n ?? 0,
    daily,
    versions,
  };
}

const esc = (s) =>
  String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

function page(s) {
  const max = Math.max(1, ...s.daily.map((d) => d.active));
  const bars = s.daily
    .map(
      (d) => `<div class="bar" title="${esc(d.day)}: ${d.active} active, ${d.new} new">
        <i style="height:${(d.active / max) * 100}%"></i><b style="height:${(d.new / max) * 100}%"></b></div>`,
    )
    .join("");
  const versions = s.versions
    .map((v) => `<tr><td>${esc(v.version)}</td><td>${v.n}</td></tr>`)
    .join("");
  const tile = (label, n) => `<div class="tile"><span>${label}</span><strong>${n}</strong></div>`;
  return `<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>Nook stats</title>
<style>
:root{--bg:#f6f6f8;--card:#fff;--text:#18181c;--dim:#6b6b78;--accent:#7c5cff;--new:#2fb37a;--line:#e4e4ea}
@media (prefers-color-scheme:dark){:root{--bg:#121216;--card:#1c1c22;--text:#f2f2f5;--dim:#9a9aa8;--line:#2c2c34}}
body{margin:0;background:var(--bg);color:var(--text);font:15px/1.4 system-ui,"Segoe UI",sans-serif}
main{max-width:860px;margin:0 auto;padding:32px 16px}
h1{font-size:22px;margin:0 0 20px}h2{font-size:15px;margin:28px 0 10px;color:var(--dim);font-weight:600}
.tiles{display:grid;grid-template-columns:repeat(auto-fit,minmax(170px,1fr));gap:12px}
.tile{background:var(--card);border:1px solid var(--line);border-radius:14px;padding:16px}
.tile span{color:var(--dim);font-size:13px}.tile strong{display:block;font-size:30px;margin-top:4px}
.chart{display:flex;align-items:flex-end;gap:3px;height:160px;background:var(--card);border:1px solid var(--line);border-radius:14px;padding:14px}
.bar{flex:1;max-width:26px;height:100%;position:relative}.bar i,.bar b{position:absolute;bottom:0;left:0;right:0;border-radius:3px 3px 0 0}
.bar i{background:var(--accent);opacity:.8}.bar b{background:var(--new)}
.legend{color:var(--dim);font-size:13px;margin-top:6px}.legend i{display:inline-block;width:10px;height:10px;border-radius:2px;margin:0 4px 0 10px}
table{width:100%;border-collapse:collapse;background:var(--card);border:1px solid var(--line);border-radius:14px;overflow:hidden}
td{padding:9px 14px;border-top:1px solid var(--line)}tr:first-child td{border-top:0}td:last-child{text-align:right}
p.note{color:var(--dim);font-size:13px;margin-top:28px}
</style></head><body><main>
<h1>Nook usage</h1>
<div class="tiles">${tile("Total installs", s.installs)}${tile("Opened today", s.active_today)}${tile("Active last 7 days", s.active_7d)}${tile("Active last 30 days", s.active_30d)}</div>
<h2>Last 30 days</h2>
<div class="chart">${bars || '<span class="legend">No data yet</span>'}</div>
<div class="legend"><i style="background:var(--accent)"></i>opened that day<i style="background:var(--new)"></i>new installs</div>
<h2>Versions in use (last 7 days)</h2>
<table>${versions || "<tr><td>No data yet</td><td></td></tr>"}</table>
<p class="note">Counts installs made by Setup that left "Send anonymous usage stats" on. Days are UTC.</p>
</main></body></html>`;
}
