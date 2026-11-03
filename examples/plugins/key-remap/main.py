"""Key Remap: pick your own shortcut for any of Nook's built-in tools.

Nook fixes those shortcuts in its code and a plugin cannot change them. So
this plugin registers the NEW key itself and, when you press it, types the
ORIGINAL shortcut for you. The original keeps working too (Nook owns it)."""

import sys

import nook
import remap

app = nook.Plugin()
entries = app.load("entries", None) or [
    {"id": i, "name": n, "old": o, "new": remap.DEFAULT_NEW.get(i, "")}
    for i, n, o in remap.DEFAULT_TOOLS]
selected = entries[0]["id"]
status = ""
errors = {}  # entry id -> why its new key could not be registered
engine = None


def active():
    return [e for e in entries if e.get("new")]


def apply():
    app.save("entries", entries)
    if engine:
        engine.set([(e["id"], e["new"], e["old"]) for e in active()])
    draw()


def on_result(result):
    errors.clear()
    errors.update({k: v for k, v in result.items() if v})
    draw()


def draw():
    n = len(active()) - len(errors)
    app.tile(nook.column([
        nook.icon("keyboard", size=22),
        nook.text(f"{n} remapped", size="small", color="dim"),
    ], align="center", justify="center"))
    cur = next(e for e in entries if e["id"] == selected)
    rows = []
    for e in entries:
        trailing = remap.pretty(e["new"]) if e.get("new") else "-"
        if e["id"] in errors:
            trailing = "! " + trailing
        rows.append(nook.item(e["id"], e["name"], subtitle="original " + remap.pretty(e["old"]),
                              trailing=trailing, selected=e["id"] == selected))
    msg = status or errors.get(selected, "")
    app.page(nook.column([
        nook.text("Pick a tool, then type its new shortcut.", color="dim"),
        nook.list_view(rows),
        nook.row([
            nook.text_input("set", "", hint=f"New shortcut for {cur['name']}, e.g. ctrl+shift+Q"),
            nook.button("clear", icon="x", tooltip="Remove the new shortcut"),
        ], align="center"),
        nook.text(msg, color="faint") if msg else nook.spacer(0),
    ], scroll=True, gap=8))


@app.on_message("event")
def on_item(e):
    global selected, status
    if e["id"] in {x["id"] for x in entries}:  # a list row
        selected, status = e["id"], ""
        draw()


@app.on("set")
def on_set(e):
    global status
    if e["kind"] != "submit":
        return
    text = (e.get("value") or "").strip()
    try:
        why = remap.check(entries, selected, text)
    except ValueError as err:
        why = str(err)
    if why:
        status = "Not set: " + why
        draw()
        return
    next(x for x in entries if x["id"] == selected)["new"] = text
    status = "Set to " + remap.pretty(text)
    apply()


@app.on("clear")
def on_clear(e):
    global status
    next(x for x in entries if x["id"] == selected)["new"] = ""
    status = "Removed"
    apply()


@app.on_message("shutdown")
def on_shutdown(msg):
    if engine:
        engine.stop()


if sys.platform == "win32":
    import engine as _engine
    engine = _engine.Engine(on_result)
    engine.set([(e["id"], e["new"], e["old"]) for e in active()])
else:
    app.log("Key Remap only works on Windows", "warn")

draw()
app.run()
