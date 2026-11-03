"""Nook plugin helper for Python 3.8+. No dependencies: copy this file next
to your plugin's main.py.

    import nook

    app = nook.Plugin()
    count = 0

    def draw():
        app.tile(nook.column([
            nook.text(str(count), size="big"),
            nook.row([nook.button("dec", icon="minus"), nook.button("inc", icon="plus")]),
        ], align="center", justify="center"))

    @app.on("inc")
    def _(event):
        global count
        count += 1
        draw()

    @app.on("dec")
    def _(event):
        global count
        count -= 1
        draw()

    draw()
    app.run()

Protocol reference: docs/plugins.html in the Nook repository.
"""

import json
import os
import queue
import sys
import threading
import time

__all__ = [
    "Plugin", "poll_event", "emit",
    "column", "row", "grid", "card", "spacer", "divider",
    "text", "icon", "image", "progress", "ring", "sparkline", "list_view", "item",
    "canvas", "button", "toggle", "slider", "text_input", "chips",
    "rect", "circle", "line", "polygon", "arc", "label", "glyph",
]

PROTOCOL = 1


def _clean(d):
    """Drops None values so Nook's defaults apply."""
    return {k: v for k, v in d.items() if v is not None}


def _node(type_, tooltip=None, **fields):
    return _clean({"type": type_, "tooltip": tooltip, **fields})


# Layout -------------------------------------------------------------------

def column(children, gap=None, align=None, justify=None, scroll=None, id=None, tooltip=None):
    """Stacks children top to bottom. align: start|center|end (across);
    justify: start|center|end (down the available height). With scroll=True,
    justify="end" keeps it scrolled to the bottom; id keys the scroll position."""
    return _node("column", tooltip, children=list(children), gap=gap, align=align,
                 justify=justify, scroll=scroll, id=id)


def row(children, gap=None, align=None, tooltip=None):
    """Lays children left to right. A spacer() pushes the rest to the right."""
    return _node("row", tooltip, children=list(children), gap=gap, align=align)


def grid(columns, children, gap=None, tooltip=None):
    return _node("grid", tooltip, columns=columns, children=list(children), gap=gap)


def card(children, padding=None, gap=None, tooltip=None):
    return _node("card", tooltip, children=list(children), padding=padding, gap=gap)


def spacer(size=None):
    """A fixed gap, or with no size: pushes what follows to the far end."""
    return _node("spacer", size=size)


def divider():
    return _node("divider")


# Content ------------------------------------------------------------------

def text(value, size=None, color=None, bold=None, wrap=None, mono=None, tooltip=None):
    """size: small|body|title|big|huge. color: text|dim|faint|accent|on_accent|
    red|orange|yellow|green|teal|blue|purple|pink|white|black|#RRGGBB[AA]."""
    return _node("text", tooltip, text=str(value), size=size, color=color, bold=bold,
                 wrap=wrap, mono=mono)


def icon(name, size=None, color=None, fill=None, tooltip=None):
    """Any Phosphor icon name from phosphoricons.com, e.g. "cloud-sun"."""
    return _node("icon", tooltip, icon=name, size=size, color=color, fill=fill)


def image(path=None, base64=None, width=None, height=None, rounding=None, tooltip=None):
    """A PNG/JPG/BMP file (relative to the plugin folder or absolute) or its
    bytes in base64."""
    return _node("image", tooltip, path=path, base64=base64, width=width, height=height,
                 rounding=rounding)


def progress(value, color=None, width=None, tooltip=None):
    return _node("progress", tooltip, value=value, color=color, width=width)


def ring(value, label=None, size=None, color=None, tooltip=None):
    return _node("ring", tooltip, value=value, label=label, size=size, color=color)


def sparkline(values, max=None, color=None, width=None, height=None, tooltip=None):
    return _node("sparkline", tooltip, values=list(values), max=max, color=color,
                 width=width, height=height)


def item(id, title, subtitle=None, icon=None, trailing=None, selected=None):
    return _clean({"id": id, "title": title, "subtitle": subtitle, "icon": icon,
                   "trailing": trailing, "selected": selected})


def list_view(items, tooltip=None):
    """Clickable rows; each click sends an event with the item's id."""
    return _node("list", tooltip, items=list(items))


def canvas(height, shapes, width=None, id=None, keys=None, tooltip=None):
    """Free drawing. With an id, clicks send {"x", "y"} in canvas points. With
    keys=True too, the canvas holds the keyboard while shown and sends "key"
    events with {"key", "repeat", "shift"}: "Up", "Space", "Enter", "Escape",
    "A", "7" (named by position on the keyboard, so WASD works anywhere)."""
    return _node("canvas", tooltip, height=height, width=width, id=id, keys=keys,
                 shapes=list(shapes))


# Controls -----------------------------------------------------------------

def button(id, label=None, icon=None, primary=None, width=None, tooltip=None):
    """Sends a "click" event. No label: a round icon button. width=0: full width."""
    return _node("button", tooltip, id=id, label=label, icon=icon, primary=primary,
                 width=width)


def toggle(id, value, label=None, tooltip=None):
    """Sends "change" with true/false."""
    return _node("toggle", tooltip, id=id, value=bool(value), label=label)


def slider(id, value, min=0, max=1, step=None, width=None, tooltip=None):
    """Sends "change" while dragging and "commit" when let go."""
    return _node("slider", tooltip, id=id, value=value, min=min, max=max, step=step,
                 width=width)


def text_input(id, value="", hint=None, width=None, live=None, tooltip=None):
    """Sends "submit" on Enter (and "change" on every edit with live=True)."""
    return _node("input", tooltip, id=id, value=value, hint=hint, width=width, live=live)


def chips(id, options, selected=0, tooltip=None):
    """Sends "change" with {"index", "label"}."""
    return _node("chips", tooltip, id=id, options=list(options), selected=selected)


# Canvas shapes --------------------------------------------------------------

def rect(x, y, w, h, fill=None, stroke=None, width=None, radius=None):
    return _clean({"type": "rect", "x": x, "y": y, "w": w, "h": h, "fill": fill,
                   "stroke": stroke, "width": width, "radius": radius})


def circle(x, y, r, fill=None, stroke=None, width=None):
    return _clean({"type": "circle", "x": x, "y": y, "r": r, "fill": fill,
                   "stroke": stroke, "width": width})


def line(points, color=None, width=None, closed=None):
    return _clean({"type": "line", "points": [list(p) for p in points], "color": color,
                   "width": width, "closed": closed})


def polygon(points, fill=None):
    return _clean({"type": "polygon", "points": [list(p) for p in points], "fill": fill})


def arc(x, y, r, start, end, color=None, width=None):
    """Degrees, clockwise from 12 o'clock."""
    return _clean({"type": "arc", "x": x, "y": y, "r": r, "start": start, "end": end,
                   "color": color, "width": width})


def label(x, y, value, size=None, color=None, anchor=None, bold=None):
    """Text on a canvas. anchor: start|center|end."""
    return _clean({"type": "text", "x": x, "y": y, "text": str(value), "size": size,
                   "color": color, "anchor": anchor, "bold": bold})


def glyph(x, y, name, size=None, color=None, fill=None):
    """An icon on a canvas, centered on (x, y)."""
    return _clean({"type": "icon", "x": x, "y": y, "icon": name, "size": size,
                   "color": color, "fill": fill})


# Talking to Nook ------------------------------------------------------------

_out_lock = threading.Lock()


def emit(message):
    """Writes one message to Nook."""
    line = json.dumps(message, separators=(",", ":"), ensure_ascii=False)
    with _out_lock:
        sys.stdout.write(line + "\n")
        sys.stdout.flush()


def poll_event():
    """In poll mode: the event that caused this run (a dict), or None."""
    raw = os.environ.get("NOOK_EVENT")
    return json.loads(raw) if raw else None


class Plugin:
    """A live-mode plugin: register handlers, then call run()."""

    def __init__(self):
        self.id = os.environ.get("NOOK_PLUGIN_ID", "dev")
        self.data_dir = os.environ.get("NOOK_DATA_DIR", os.path.join(os.getcwd(), ".data"))
        os.makedirs(self.data_dir, exist_ok=True)
        self.dark = os.environ.get("NOOK_THEME", "dark") == "dark"
        self.accent = os.environ.get("NOOK_ACCENT", "#7C5CFF")
        self.tile_size = None
        self.tile_width = self.tile_height = None
        self.page_open = False
        self._events = {}
        self._messages = {}
        self._timers = []
        self._inbox = queue.Queue()
        self._running = True

    # Registering handlers

    def on(self, event_id):
        """Decorator: handler(event) for a control's events. event has
        "id", "kind" (click, change, submit, commit) and maybe "value"."""
        def deco(fn):
            self._events[event_id] = fn
            return fn
        return deco

    def on_message(self, type_):
        """Decorator for Nook's own messages: hello, theme, layout, tile_click,
        page_open, page_close, hotkey, files, shutdown."""
        def deco(fn):
            self._messages.setdefault(type_, []).append(fn)
            return fn
        return deco

    def every(self, seconds, fn=None):
        """Calls fn every `seconds` (also usable as a decorator)."""
        def deco(f):
            self._timers.append([time.monotonic() + seconds, seconds, f])
            return f
        return deco(fn) if fn else deco

    # Sending

    def tile(self, view):
        emit({"type": "tile", "view": view})

    def page(self, view, actions=None):
        """actions: [{"id": "refresh", "icon": "arrow-clockwise", "tooltip": "..."}]"""
        emit(_clean({"type": "page", "view": view, "actions": actions}))

    def badge_ring(self, progress, label=""):
        emit({"type": "badge", "badge": {"kind": "ring", "progress": progress, "label": label}})

    def badge_glyph(self, name):
        emit({"type": "badge", "badge": {"kind": "glyph", "icon": name}})

    def badge_pulse(self):
        emit({"type": "badge", "badge": {"kind": "pulse"}})

    def clear_badge(self):
        emit({"type": "badge", "badge": None})

    def active(self, on=True):
        emit({"type": "active", "active": bool(on)})

    def open_page(self):
        emit({"type": "open_page"})

    def close_page(self):
        emit({"type": "close_page"})

    def show(self):
        """Opens the notch, pinned, on this plugin's page."""
        emit({"type": "show"})

    def copy(self, value):
        emit({"type": "copy", "text": str(value)})

    def open(self, target):
        """Opens a URL, file or folder with its default app."""
        emit({"type": "open", "target": target})

    def log(self, message, level="info"):
        emit({"type": "log", "level": level, "message": str(message)})

    # Storage in the plugin's data folder

    def load(self, name, default=None):
        try:
            with open(os.path.join(self.data_dir, name + ".json"), encoding="utf-8") as f:
                return json.load(f)
        except (OSError, ValueError):
            return default

    def save(self, name, value):
        path = os.path.join(self.data_dir, name + ".json")
        tmp = path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(value, f, ensure_ascii=False, indent=1)
        os.replace(tmp, path)

    # Loop

    def _read(self):
        for line in sys.stdin:
            line = line.strip()
            if line:
                try:
                    self._inbox.put(json.loads(line))
                except ValueError:
                    pass
        self._inbox.put({"type": "shutdown"})

    def stop(self):
        self._running = False

    def _dispatch(self, msg):
        t = msg.get("type")
        if t == "hello":
            theme = msg.get("theme", {})
            self.dark = theme.get("dark", self.dark)
            self.accent = theme.get("accent", self.accent)
        elif t == "theme":
            self.dark = msg.get("dark", self.dark)
            self.accent = msg.get("accent", self.accent)
        elif t == "layout" and msg.get("target") == "tile":
            self.tile_size = msg.get("size")
            self.tile_width, self.tile_height = msg.get("width"), msg.get("height")
        elif t == "page_open":
            self.page_open = True
        elif t == "page_close":
            self.page_open = False
        if t == "event":
            fn = self._events.get(msg.get("id"))
            if fn:
                fn(msg)
        for fn in self._messages.get(t, []):
            fn(msg)
        if t == "shutdown":
            self._running = False

    def run(self):
        """Handles messages and timers until Nook says goodbye."""
        threading.Thread(target=self._read, daemon=True).start()
        while self._running:
            timeout = None
            if self._timers:
                timeout = max(0.0, min(t[0] for t in self._timers) - time.monotonic())
            try:
                msg = self._inbox.get(timeout=timeout)
            except queue.Empty:
                msg = None
            try:
                if msg is not None:
                    self._dispatch(msg)
                now = time.monotonic()
                for t in self._timers:
                    if now >= t[0]:
                        t[0] = now + t[1]
                        t[2]()
            except Exception as e:  # keep the plugin alive; show the error in Nook's log
                import traceback
                traceback.print_exc()
                self.log(f"{type(e).__name__}: {e}", "error")
