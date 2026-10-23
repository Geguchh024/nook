"""To-do: a page with an input, a filter and a list; the tile shows what's
next; the pill pulses while tasks are open. Tasks live in NOOK_DATA_DIR."""

import os
import time

import nook

app = nook.Plugin()
tasks = app.load("tasks", [])  # [{"id", "text", "done"}]
view = 0  # 0 open, 1 done, 2 all


def save():
    app.save("tasks", tasks)
    draw()


def open_tasks():
    return [t for t in tasks if not t["done"]]


def draw():
    pending = open_tasks()
    # Tile: count on the left, the next task on the right.
    app.tile(nook.row([
        nook.ring(1 - len(pending) / max(len(tasks), 1), label=str(len(pending)), size=34,
                  tooltip="Open tasks"),
        nook.column([
            nook.text("To-do", size="small", color="dim", bold=True),
            nook.text(pending[0]["text"] if pending else "All done", wrap=False),
        ], gap=0),
    ], gap=8, align="center"))

    shown = [t for t in tasks if (view == 2) or (t["done"] == (view == 1))]
    rows = [nook.item(t["id"], t["text"],
                      icon="check-circle" if t["done"] else "circle",
                      trailing=t.get("when", ""))
            for t in shown]
    app.page(nook.column([
        nook.row([
            nook.text_input("new", "", hint="Add a task and press Enter"),
        ]),
        nook.row([
            nook.chips("view", ["Open", "Done", "All"], view),
            nook.spacer(),
            nook.button("clear", label="Clear done", icon="broom"),
        ], align="center"),
        nook.list_view(rows) if rows else nook.text("Nothing here.", color="faint"),
    ], scroll=True), actions=[
        {"id": "copy", "icon": "copy", "tooltip": "Copy open tasks"},
    ])
    if pending:
        app.badge_glyph("check-square")
    else:
        app.clear_badge()


def add(text):
    text = text.strip()
    if text:
        tasks.append({"id": f"t{time.time_ns()}", "text": text, "done": False,
                      "when": time.strftime("%d %b")})
        save()


@app.on("new")
def on_new(e):
    if e["kind"] == "submit":
        add(e.get("value", ""))


@app.on("view")
def on_view(e):
    global view
    view = e["value"]["index"]
    draw()


@app.on("clear")
def on_clear(e):
    tasks[:] = open_tasks()
    save()


@app.on("copy")
def on_copy(e):
    app.copy("\n".join("- " + t["text"] for t in open_tasks()))


@app.on_message("event")
def on_item(e):
    # List rows send their item id; flip that task.
    for t in tasks:
        if t["id"] == e["id"]:
            t["done"] = not t["done"]
            save()


@app.on_message("hotkey")
def on_hotkey(msg):
    app.show()


@app.on_message("files")
def on_files(msg):
    for p in msg["paths"]:
        add("Look at " + os.path.basename(p))


draw()
app.run()
