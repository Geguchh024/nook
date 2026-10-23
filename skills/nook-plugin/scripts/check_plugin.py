#!/usr/bin/env python3
"""Checks a Nook plugin the way Nook itself would load and run it.

    python check_plugin.py PLUGIN_DIR [--event ID:KIND[:VALUE]]... [options]

1. Validates plugin.toml with Nook's rules (unknown keys, id, run, size,
   mode, interval, hotkey, icon).
2. Resolves the program in `run` the way Nook does (plugin folder, then PATH).
3. Runs the plugin with Nook's environment. Live mode: sends hello, layout,
   each --event, page_open and shutdown on stdin. Poll mode: one run, then
   one run per --event with NOOK_EVENT set.
4. Checks every line printed against the protocol: message types, required
   fields, field types, component types, colors, icon names, control ids.

Exit code 0 when there are no errors (warnings are allowed), 1 otherwise.
Python 3.8+, no dependencies. Plugin data goes to a temporary folder unless
--data-dir is given, so checking never touches the plugin's real data.
"""

import argparse
import json
import os
import queue
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ICONS_FILE = HERE.parent / "references" / "icons.txt"

MANIFEST_KEYS = {
    "id", "name", "icon", "size", "run", "mode", "interval", "hotkey",
    "accepts_files", "page_size", "description", "author", "version",
}
TILE_SIZES = {"small": (44, 44), "wide": (104, 36), "large": (104, 104)}
PAGE_SIZE = (536, 168)
TAKEN_HOTKEYS = {"ctrl+alt+" + k for k in "cdfklnpqrst"}
EXAMPLE_HOTKEYS = {"ctrl+alt+y": "todo"}
POLL_TIMEOUT = 30

THEME_COLORS = {"text", "dim", "faint", "accent", "on_accent"}
FIXED_COLORS = {"red", "orange", "yellow", "green", "teal", "blue", "purple",
                "pink", "white", "black", "transparent"}
HEX = re.compile(r"^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$")
ALIGN = ("enum", ("start", "center", "end"))
TEXT_SIZE = ("enum", ("small", "body", "title", "big", "huge"))

# Field specs: a type name, or ("enum", values). A trailing "?" means JSON
# null is accepted (Option<T> in Nook); without it, null is an error.
NODES = {
    "column": ({}, {"children": "nodes", "gap": "num?", "align": ALIGN, "justify": ALIGN, "scroll": "bool"}),
    "row": ({}, {"children": "nodes", "gap": "num?", "align": ALIGN}),
    "grid": ({"columns": "uint"}, {"children": "nodes", "gap": "num?"}),
    "card": ({}, {"children": "nodes", "padding": "num?", "gap": "num?"}),
    "spacer": ({}, {"size": "num?"}),
    "divider": ({}, {}),
    "text": ({"text": "str"}, {"size": TEXT_SIZE, "color": "color?", "bold": "bool", "wrap": "bool", "mono": "bool"}),
    "icon": ({"icon": "icon"}, {"size": "num?", "color": "color?", "fill": "bool"}),
    "image": ({}, {"path": "str?", "base64": "str?", "width": "num?", "height": "num?", "rounding": "num"}),
    "progress": ({"value": "num"}, {"color": "color?", "width": "num?"}),
    "ring": ({"value": "num"}, {"label": "str", "size": "num?", "color": "color?"}),
    "sparkline": ({"values": "nums"}, {"max": "num?", "color": "color?", "width": "num?", "height": "num?"}),
    "list": ({"items": "items"}, {}),
    "canvas": ({"height": "num"}, {"id": "str?", "width": "num?", "shapes": "shapes"}),
    "button": ({"id": "str"}, {"label": "str", "icon": "icon?", "primary": "bool", "width": "num?"}),
    "toggle": ({"id": "str", "value": "bool"}, {"label": "str?"}),
    "slider": ({"id": "str", "value": "num"}, {"min": "num", "max": "num", "step": "num?", "width": "num?"}),
    "input": ({"id": "str"}, {"value": "str", "hint": "str", "width": "num?", "live": "bool"}),
    "chips": ({"id": "str", "options": "strs"}, {"selected": "uint"}),
}
SHAPES = {
    "rect": ({"x": "num", "y": "num", "w": "num", "h": "num"}, {"radius": "num", "fill": "color?", "stroke": "color?", "width": "num"}),
    "circle": ({"x": "num", "y": "num", "r": "num"}, {"fill": "color?", "stroke": "color?", "width": "num"}),
    "line": ({"points": "points"}, {"color": "color?", "width": "num", "closed": "bool"}),
    "polygon": ({"points": "points"}, {"fill": "color?"}),
    "arc": ({"x": "num", "y": "num", "r": "num", "start": "num", "end": "num"}, {"color": "color?", "width": "num"}),
    "text": ({"x": "num", "y": "num", "text": "str"}, {"size": "num", "color": "color?", "anchor": ALIGN, "bold": "bool"}),
    "icon": ({"x": "num", "y": "num", "icon": "icon"}, {"size": "num", "color": "color?", "fill": "bool"}),
}
LIST_ITEM = ({"id": "str", "title": "str"}, {"subtitle": "str", "icon": "icon?", "trailing": "str", "selected": "bool"})
ACTION = ({"id": "str", "icon": "icon"}, {"tooltip": "str"})
MESSAGES = {
    "tile": ({"view": "node"}, {}),
    "page": ({"view": "node"}, {"actions": "actions"}),
    "badge": ({"badge": "badge?"}, {}),
    "active": ({"active": "bool"}, {}),
    "open_page": ({}, {}),
    "close_page": ({}, {}),
    "show": ({}, {}),
    "copy": ({"text": "str"}, {}),
    "open": ({"target": "str"}, {}),
    "log": ({"message": "str"}, {"level": "str"}),
}
CONTROLS = {"button", "toggle", "slider", "input", "chips"}
BIG_CONTROLS = {"input", "list", "chips", "slider"}


class Report:
    def __init__(self):
        self.errors, self.warnings, self.notes = [], [], []

    def error(self, msg):
        self.errors.append(msg)

    def warn(self, msg):
        if msg not in self.warnings:
            self.warnings.append(msg)

    def note(self, msg):
        self.notes.append(msg)


# Manifest ----------------------------------------------------------------

def load_toml(text):
    try:
        import tomllib  # Python 3.11+
        return tomllib.loads(text)
    except ImportError:
        pass
    try:
        import tomli
        return tomli.loads(text)
    except ImportError:
        return mini_toml(text)


def mini_toml(text):
    """Enough TOML for plugin.toml (key = string/number/bool/array) on old Pythons."""
    out = {}
    for n, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            raise ValueError(f"line {n}: expected key = value")
        key, value = (s.strip() for s in line.split("=", 1))
        value = re.sub(r"\s+#[^\"']*$", "", value)
        value = value.replace("true", "True").replace("false", "False") if not value.startswith(("\"", "'")) else value
        try:
            import ast
            out[key.strip("\"'")] = ast.literal_eval(value)
        except (ValueError, SyntaxError):
            raise ValueError(f"line {n}: can't read value {value!r}")
    return out


def normalize_icon(name):
    key = []
    for i, c in enumerate(name.strip()):
        if c in "_ ":
            key.append("-")
        elif c.isupper():
            if i > 0 and (not key or key[-1] != "-"):
                key.append("-")
            key.append(c.lower())
        else:
            key.append(c)
    return "".join(key)


ICONS = set()


def icon_ok(name):
    return not ICONS or normalize_icon(name) in ICONS


def check_hotkey(hk, plugin_id, r):
    parts = [p.strip() for p in hk.split("+")]
    mods = {"ctrl", "control", "alt", "option", "shift", "super", "cmd", "command", "meta"}
    if len(parts) < 2 or not all(p.lower() in mods for p in parts[:-1]) or parts[-1].lower() in mods:
        r.error(f'hotkey "{hk}": write modifiers then one key, e.g. "ctrl+alt+KeyW"')
        return
    key = parts[-1]
    norm = key.lower()
    norm = norm[3:] if norm.startswith("key") and len(norm) == 4 else norm
    combo = "+".join(p.lower().replace("control", "ctrl") for p in parts[:-1]) + "+" + norm
    if combo in TAKEN_HOTKEYS:
        r.warn(f'hotkey "{hk}" is already used by a built-in Nook tool and will not register; pick another key')
    elif EXAMPLE_HOTKEYS.get(combo, plugin_id) != plugin_id:
        r.warn(f'hotkey "{hk}" is used by the {EXAMPLE_HOTKEYS[combo]} example plugin; pick another key')


def check_manifest(dir_, r):
    path = dir_ / "plugin.toml"
    if not path.is_file():
        r.error(f"no plugin.toml in {dir_}")
        return None
    try:
        m = load_toml(path.read_text(encoding="utf-8"))
    except UnicodeDecodeError:
        r.error("plugin.toml is not UTF-8")
        return None
    except Exception as e:  # tomllib.TOMLDecodeError or ValueError
        r.error(f"plugin.toml does not parse: {e}")
        return None

    unknown = sorted(set(m) - MANIFEST_KEYS)
    for k in unknown:
        r.error(f'plugin.toml: unknown key "{k}". Nook rejects the whole manifest and skips the plugin. '
                f"Allowed: {', '.join(sorted(MANIFEST_KEYS))}")
    pid = m.get("id")
    if not isinstance(pid, str) or not re.fullmatch(r"[a-z0-9_-]{1,48}", pid or ""):
        r.error(f"plugin.toml: id {pid!r} must be 1-48 characters of a-z, 0-9, - and _")
    if not isinstance(m.get("name"), str) or not m.get("name"):
        r.error("plugin.toml: name is required")
    run = m.get("run")
    if not isinstance(run, list) or not run or not all(isinstance(a, str) for a in run) or not run[0].strip():
        r.error('plugin.toml: run must be a list of strings naming a program, e.g. run = ["python", "main.py"]')
    size = m.get("size", "small")
    if size not in TILE_SIZES:
        r.error(f'plugin.toml: size "{size}" must be small, wide or large')
    mode = m.get("mode", "live")
    if mode not in ("live", "poll"):
        r.error(f'plugin.toml: mode "{mode}" must be live or poll')
    interval = m.get("interval", 60)
    if not isinstance(interval, (int, float)) or isinstance(interval, bool):
        r.error("plugin.toml: interval must be a number of seconds")
    elif mode == "poll" and interval < 1:
        r.error("plugin.toml: interval must be at least 1 second")
    elif mode == "live" and "interval" in m:
        r.warn("plugin.toml: interval only applies to poll mode")
    icon = m.get("icon", "puzzle-piece")
    if not isinstance(icon, str):
        r.error("plugin.toml: icon must be a string")
    elif not icon_ok(icon):
        r.warn(f'plugin.toml: icon "{icon}" is not a Phosphor name Nook knows; it shows as "?" (see references/icons.txt)')
    if "hotkey" in m:
        if isinstance(m["hotkey"], str):
            check_hotkey(m["hotkey"], pid, r)
        else:
            r.error("plugin.toml: hotkey must be a string")
    if "accepts_files" in m and not isinstance(m["accepts_files"], bool):
        r.error("plugin.toml: accepts_files must be true or false")
    ps = m.get("page_size")
    if ps is not None and (not isinstance(ps, list) or len(ps) != 2
                           or not all(isinstance(v, (int, float)) and not isinstance(v, bool) for v in ps)):
        r.error("plugin.toml: page_size must be [width, height] in points")
    for k in ("description", "author", "version"):
        if k in m and not isinstance(m[k], str):
            r.error(f"plugin.toml: {k} must be a string")
    if not m.get("version"):
        r.note("plugin.toml has no version; the marketplace and logs show it, so set one before publishing")
    if dir_.name != pid and isinstance(pid, str):
        r.note(f'folder is "{dir_.name}" but id is "{pid}"; installs from the website use the id as the folder name')
    return None if r.errors else m


def resolve_program(dir_, run, r):
    first = run[0]
    local = dir_ / first
    if ("/" in first or "\\" in first or local.is_file()) and local.exists():
        return str(local)
    found = shutil.which(first)
    if not found:
        r.error(f'run: "{first}" is neither a file in the plugin folder nor on PATH. '
                f"Nook will show the plugin as Stopped. (Is it installed? On Windows, Python is often "
                f'"py" or "python", Node is "node".)')
        return None
    return found


# Protocol validation -------------------------------------------------------

def is_num(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def check_field(where, name, spec, value, r, ids, depth):
    nullable = isinstance(spec, str) and spec.endswith("?")
    kind = spec[:-1] if nullable else spec
    if value is None:
        if not nullable:
            r.error(f"{where}: {name} is null; omit it or give a value (null is only allowed for optional values)")
        return
    if isinstance(kind, tuple):
        if value not in kind[1]:
            r.error(f'{where}: {name} "{value}" must be one of {", ".join(kind[1])}')
        return
    if kind == "str":
        if not isinstance(value, str):
            r.error(f"{where}: {name} must be a string, got {json.dumps(value)[:40]} (send text as a string)")
    elif kind == "num":
        if not is_num(value):
            r.error(f"{where}: {name} must be a number, got {json.dumps(value)[:40]}")
    elif kind == "uint":
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            r.error(f"{where}: {name} must be a whole number >= 0 (not {json.dumps(value)[:20]}; 2.0 is not accepted)")
    elif kind == "bool":
        if not isinstance(value, bool):
            r.error(f"{where}: {name} must be true or false, got {json.dumps(value)[:40]}")
    elif kind == "color":
        if not isinstance(value, str):
            r.error(f"{where}: {name} must be a color string")
        elif value.strip() not in THEME_COLORS | FIXED_COLORS and not HEX.match(value.strip()):
            r.warn(f'{where}: color "{value}" is unknown and falls back to the default. '
                   f"Use text, dim, faint, accent, on_accent, a fixed name or #RRGGBB")
    elif kind == "icon":
        if not isinstance(value, str):
            r.error(f"{where}: {name} must be an icon name string")
        elif not icon_ok(value):
            r.warn(f'{where}: icon "{value}" is not a known Phosphor name and shows as "?" (see references/icons.txt)')
    elif kind == "nums":
        if not isinstance(value, list) or not all(is_num(v) for v in value):
            r.error(f"{where}: {name} must be an array of numbers")
    elif kind == "strs":
        if not isinstance(value, list) or not all(isinstance(v, str) for v in value):
            r.error(f"{where}: {name} must be an array of strings")
    elif kind == "points":
        if not isinstance(value, list) or not all(isinstance(p, list) and len(p) == 2 and all(is_num(c) for c in p) for p in value):
            r.error(f"{where}: {name} must be [[x, y], ...] pairs of numbers")
    elif kind == "node":
        check_node(value, f"{where}.{name}", r, ids, depth + 1)
    elif kind == "nodes":
        if not isinstance(value, list):
            r.error(f"{where}: {name} must be an array of components")
        else:
            for i, child in enumerate(value):
                check_node(child, f"{where}.{name}[{i}]", r, ids, depth + 1)
    elif kind == "items":
        if not isinstance(value, list):
            r.error(f"{where}: items must be an array")
        else:
            for i, it in enumerate(value):
                check_object(it, LIST_ITEM, f"{where}.items[{i}]", r, ids, depth)
                if isinstance(it, dict) and isinstance(it.get("id"), str):
                    ids.append(it["id"])
    elif kind == "shapes":
        if not isinstance(value, list):
            r.error(f"{where}: shapes must be an array")
        else:
            for i, s in enumerate(value):
                w = f"{where}.shapes[{i}]"
                if not isinstance(s, dict) or not isinstance(s.get("type"), str):
                    r.error(f"{w}: a shape must be an object with a type")
                elif s["type"] not in SHAPES:
                    r.warn(f'{w}: unknown shape "{s["type"]}" is drawn as nothing ({", ".join(SHAPES)})')
                else:
                    check_object(s, SHAPES[s["type"]], f'{w}({s["type"]})', r, ids, depth, skip={"type"})
    elif kind == "actions":
        if not isinstance(value, list):
            r.error(f"{where}: actions must be an array")
        else:
            for i, a in enumerate(value):
                check_object(a, ACTION, f"{where}.actions[{i}]", r, ids, depth)
    elif kind == "badge":
        w = f"{where}.badge"
        if not isinstance(value, dict):
            r.error(f"{w}: must be an object or null")
        elif value.get("kind") == "ring":
            check_object(value, ({"progress": "num"}, {"label": "str"}), w, r, ids, depth, skip={"kind"})
        elif value.get("kind") == "glyph":
            check_object(value, ({"icon": "icon"}, {}), w, r, ids, depth, skip={"kind"})
        elif value.get("kind") == "pulse":
            pass
        else:
            r.error(f'{w}: kind must be "ring", "glyph" or "pulse"')


def check_object(obj, spec, where, r, ids, depth, skip=frozenset()):
    if not isinstance(obj, dict):
        r.error(f"{where}: must be a JSON object")
        return
    required, optional = spec
    for name, s in required.items():
        if name not in obj:
            r.error(f"{where}: missing required field {name} (Nook skips the whole message)")
        else:
            check_field(where, name, s, obj[name], r, ids, depth)
    for name, value in obj.items():
        if name in required or name in skip or name == "tooltip":
            continue
        if name in optional:
            check_field(where, name, optional[name], value, r, ids, depth)
        else:
            known = sorted(set(required) | set(optional))
            r.warn(f'{where}: field "{name}" is not used by Nook and is ignored (typo?). Known: {", ".join(known) or "none"}')
    if "tooltip" in obj and obj["tooltip"] is not None and not isinstance(obj["tooltip"], str):
        r.error(f"{where}: tooltip must be a string")


def check_node(node, where, r, ids, depth=0):
    if depth > 64:
        r.error(f"{where}: view is nested too deep")
        return
    if not isinstance(node, dict) or not isinstance(node.get("type"), str):
        r.error(f"{where}: a component must be an object with a string type")
        return
    t = node["type"]
    if t not in NODES:
        r.warn(f'{where}: unknown component "{t}" is drawn as nothing. Known: {", ".join(NODES)}')
        return
    check_object(node, NODES[t], f"{where}({t})", r, ids, depth, skip={"type"})
    if t in CONTROLS and isinstance(node.get("id"), str):
        ids.append(node["id"])
    if t == "canvas" and isinstance(node.get("id"), str):
        ids.append(node["id"])
    if t == "image" and not node.get("path") and not node.get("base64"):
        r.warn(f"{where}(image): give path or base64, or nothing is drawn")


def node_types(node, acc):
    if isinstance(node, dict):
        acc.add(node.get("type"))
        for c in node.get("children") or []:
            node_types(c, acc)
    return acc


def check_message(msg, label, r, stats, tile_size):
    if not isinstance(msg, dict):
        r.error(f"{label}: each line must be a JSON object")
        return
    t = msg.get("type")
    if not isinstance(t, str) or not t:
        r.error(f'{label}: message has no "type"')
        return
    if t not in MESSAGES:
        # A bare component is shorthand for a tile.
        msg, t = {"type": "tile", "view": msg}, "tile"
        stats["bare"] += 1
    ids = []
    errors_before = len(r.errors)
    check_object(msg, MESSAGES[t], f"{label} {t}", r, ids, 0, skip={"type"})
    stats["types"][t] = stats["types"].get(t, 0) + 1
    if len(r.errors) == errors_before and t in ("tile", "page"):
        stats["valid_" + t] += 1
    dup = sorted({i for i in ids if ids.count(i) > 1})
    if dup:
        r.warn(f"{label} {t}: control ids used more than once: {', '.join(dup)}. Each id should be unique within a view")
    stats["ids"].update(ids)
    if t == "tile" and tile_size in ("small", "wide"):
        crowded = node_types(msg.get("view"), set()) & BIG_CONTROLS
        if crowded:
            r.warn(f"{label} tile: {', '.join(sorted(crowded))} on a {tile_size} tile "
                   f"({TILE_SIZES[tile_size][0]}x{TILE_SIZES[tile_size][1]} pt) is hard to use; put it on the page")


def parse_line(line, label, r, stats, tile_size):
    try:
        msg = json.loads(line)
    except ValueError as e:
        r.error(f"{label}: not JSON ({e.msg}): {line[:80]!r}. Debug output belongs on stderr, not stdout")
        return
    check_message(msg, label, r, stats, tile_size)


# Running ----------------------------------------------------------------

EVENT_SPEC = re.compile(r"^(.+?):(click|change|submit|commit)(?::(.*))?$", re.S)


def parse_event(spec):
    """ID:KIND[:VALUE]; the id may itself contain colons ("drive:C:")."""
    m = EVENT_SPEC.match(spec)
    if not m:
        raise argparse.ArgumentTypeError(
            "use ID:KIND or ID:KIND:VALUE with KIND click, change, submit or commit, "
            "e.g. inc:click or city:submit:Paris")
    ev = {"type": "event", "id": m.group(1), "kind": m.group(2)}
    if m.group(3) is not None:
        try:
            ev["value"] = json.loads(m.group(3))
        except ValueError:
            ev["value"] = m.group(3)
    return ev


def parse_message(spec):
    """A raw message: JSON, or just a type such as hotkey or tile_click."""
    try:
        msg = json.loads(spec)
    except ValueError:
        msg = {"type": spec}
    if not isinstance(msg, dict) or "type" not in msg:
        raise argparse.ArgumentTypeError('give a type (hotkey) or a JSON message ({"type":"files","paths":["C:/a.txt"]})')
    return msg


def environment(dir_, m, data_dir, mode):
    env = dict(os.environ)
    env.update({
        "NOOK_PLUGIN_ID": m["id"], "NOOK_PLUGIN_DIR": str(dir_), "NOOK_DATA_DIR": str(data_dir),
        "NOOK_PROTOCOL": "1", "NOOK_VERSION": "0.1.0", "NOOK_MODE": mode,
        "NOOK_THEME": "dark", "NOOK_ACCENT": "#7C5CFF",
        "PYTHONUNBUFFERED": "1", "PYTHONIOENCODING": "utf-8",
    })
    env.pop("NOOK_EVENT", None)
    return env


def reader(stream, q, tag):
    for raw in iter(stream.readline, b""):
        q.put((tag, time.monotonic(), raw))
    q.put((tag, time.monotonic(), None))


def run_live(dir_, m, program, data_dir, events, seconds, r, stats):
    size = m.get("size", "small")
    w, h = TILE_SIZES.get(size, (44, 44))
    proc = subprocess.Popen([program] + m["run"][1:], cwd=dir_, env=environment(dir_, m, data_dir, "live"),
                            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    q = queue.Queue()
    for stream, tag in ((proc.stdout, "out"), (proc.stderr, "err")):
        threading.Thread(target=reader, args=(stream, q, tag), daemon=True).start()
    stderr_tail, lines, closed = [], 0, {"out": False, "err": False}
    exited_early = False

    def send(msg):
        try:
            proc.stdin.write((json.dumps(msg) + "\n").encode("utf-8"))
            proc.stdin.flush()
            return True
        except OSError:
            return False

    def pump(duration, phase):
        nonlocal lines, exited_early
        end = time.monotonic() + duration
        got = 0
        while time.monotonic() < end:
            try:
                tag, _, raw = q.get(timeout=max(0.0, end - time.monotonic()))
            except queue.Empty:
                break
            if raw is None:
                closed[tag] = True
                if tag == "out" and phase != "shutdown":
                    exited_early = True
                continue
            if tag == "err":
                stderr_tail.append(raw.decode("utf-8", "replace").rstrip())
                continue
            try:
                line = raw.decode("utf-8").strip()
            except UnicodeDecodeError:
                r.error(f"{phase}: output is not UTF-8. Set your stdout encoding to UTF-8")
                continue
            if not line:
                continue
            lines += 1
            got += 1
            stats["timeline"].append(phase)
            parse_line(line, f"[{phase} line {lines}]", r, stats, size)
        return got

    hello = {"type": "hello", "protocol": 1, "nook_version": "0.1.0", "plugin_id": m["id"],
             "data_dir": str(data_dir), "theme": {"dark": True, "accent": "#7C5CFF"}}
    send(hello)
    send({"type": "layout", "target": "tile", "size": size, "width": w, "height": h})
    idle = pump(seconds, "start")
    if idle > 10 * seconds:
        r.warn(f"printed {idle} messages in {seconds:g} s with no events: a redraw loop keeps Nook busy. "
               f"Redraw on changes only (clocks: once a second at most)")
    for ev in events:
        if exited_early:
            break
        send(ev)
        name = f"event {ev['id']}:{ev['kind']}" if ev["type"] == "event" else ev["type"]
        got = pump(1.0, name)
        if got == 0:
            r.note(f"no output after {name} (fine if nothing should change)")
        if ev["type"] == "event" and ev["id"] not in stats["ids"] and not any(ev["id"].startswith(i.split(":")[0] + ":") for i in stats["ids"]):
            r.note(f'event id "{ev["id"]}" does not match any control id the plugin has sent')
    if not exited_early:
        send({"type": "page_open"})
        send({"type": "layout", "target": "page", "size": "page", "width": PAGE_SIZE[0], "height": PAGE_SIZE[1]})
        pump(1.0, "page_open")
        send({"type": "page_close"})
        send({"type": "shutdown"})
        try:
            proc.stdin.close()
        except OSError:
            pass
        t0 = time.monotonic()
        pump(0.6, "shutdown")
        try:
            proc.wait(timeout=max(0.0, 0.6 - (time.monotonic() - t0)))
        except subprocess.TimeoutExpired:
            r.warn("still running 0.4 s after shutdown: Nook kills it then, so save state as soon as shutdown arrives")
    if proc.poll() is None:
        proc.kill()
        proc.wait()
    while True:  # drain stderr
        try:
            tag, _, raw = q.get(timeout=0.2)
        except queue.Empty:
            break
        if tag == "err" and raw:
            stderr_tail.append(raw.decode("utf-8", "replace").rstrip())
    if exited_early:
        r.error(f"the program exited while running (code {proc.returncode}). A live plugin must keep running; "
                f"Nook restarts it 3 times a minute, then shows Stopped. Last stderr:\n    "
                + "\n    ".join(stderr_tail[-6:] or ["(nothing)"]))
    return stderr_tail


def run_poll(dir_, m, program, data_dir, events, r, stats):
    size = m.get("size", "small")
    stderr_tail = []
    for ev in [None] + events:
        if ev is None:
            phase = "first run"
        elif ev["type"] == "event":
            phase = f"run with NOOK_EVENT {ev['id']}:{ev['kind']}"
        else:
            phase = f"run with NOOK_EVENT {ev['type']}"
        env = environment(dir_, m, data_dir, "poll")
        if ev is not None:
            env["NOOK_EVENT"] = json.dumps(ev)
        t0 = time.monotonic()
        try:
            done = subprocess.run([program] + m["run"][1:], cwd=dir_, env=env, stdin=subprocess.DEVNULL,
                                  capture_output=True, timeout=POLL_TIMEOUT,
                                  creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        except subprocess.TimeoutExpired:
            r.error(f"{phase}: took longer than {POLL_TIMEOUT} s; Nook kills poll runs at {POLL_TIMEOUT} s")
            continue
        took = time.monotonic() - t0
        if took > 10:
            r.warn(f"{phase}: took {took:.1f} s; every click waits for a whole run")
        err = done.stderr.decode("utf-8", "replace")
        stderr_tail += [l for l in err.splitlines() if l.strip()]
        try:
            out = done.stdout.decode("utf-8").strip()
        except UnicodeDecodeError:
            r.error(f"{phase}: output is not UTF-8 (PowerShell: [Console]::OutputEncoding = [Text.Encoding]::UTF8)")
            continue
        if done.returncode != 0 and not out:
            r.error(f"{phase}: exit code {done.returncode} with no output. Last stderr: "
                    + (stderr_tail[-1] if stderr_tail else "(nothing)"))
            continue
        if not out:
            r.warn(f"{phase}: printed nothing, so the tile keeps showing the previous view")
            continue
        stats["timeline"].append(phase)
        try:
            # Nook first tries the whole output as one JSON document.
            check_message(json.loads(out), f"[{phase}]", r, stats, size)
        except ValueError:
            for i, line in enumerate(l.strip() for l in out.splitlines()):
                if line:
                    parse_line(line, f"[{phase} line {i + 1}]", r, stats, size)
    return stderr_tail


def main():
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(errors="replace")
    ap = argparse.ArgumentParser(description="Check a Nook plugin the way Nook runs it.")
    ap.add_argument("plugin_dir", type=Path)
    ap.add_argument("--event", action="append", type=parse_event, default=[],
                    help="send an event, ID:KIND[:VALUE] (VALUE as JSON if it parses, else a string). Repeatable.")
    ap.add_argument("--message", action="append", type=parse_message, default=[], dest="event",
                    help="send any other message after the events, e.g. hotkey, tile_click or a JSON object. Repeatable.")
    ap.add_argument("--seconds", type=float, default=2.0, help="how long to watch a live plugin before events (default 2)")
    ap.add_argument("--data-dir", type=Path, help="NOOK_DATA_DIR to use (default: a temporary folder)")
    ap.add_argument("--no-run", action="store_true", help="only check plugin.toml")
    args = ap.parse_args()

    if ICONS_FILE.is_file():
        ICONS.update(l.strip() for l in ICONS_FILE.read_text(encoding="utf-8").splitlines() if l.strip())

    r = Report()
    dir_ = args.plugin_dir.resolve()
    m = check_manifest(dir_, r)
    program = resolve_program(dir_, m["run"], r) if m else None
    stats = {"types": {}, "valid_tile": 0, "valid_page": 0, "bare": 0, "ids": set(), "timeline": []}
    stderr_tail = []

    if m and program and not args.no_run:
        tmp = None
        data_dir = args.data_dir
        if data_dir is None:
            tmp = tempfile.TemporaryDirectory(prefix=f"nook-check-{m['id']}-")
            data_dir = Path(tmp.name)
        data_dir.mkdir(parents=True, exist_ok=True)
        try:
            if m.get("mode", "live") == "poll":
                stderr_tail = run_poll(dir_, m, program, data_dir, args.event, r, stats)
            else:
                stderr_tail = run_live(dir_, m, program, data_dir, args.event, args.seconds, r, stats)
        except OSError as e:
            r.error(f"could not start {m['run']}: {e}")
        finally:
            if tmp:
                tmp.cleanup()
        if not r.errors or stats["types"]:
            if stats["valid_tile"] == 0:
                r.warn("no valid tile was sent, so Nook shows the default tile (icon and name only)")
            if stats["types"].get("show", 0) > 1:
                r.warn('sent "show" more than once; it pops the notch open over the user\'s work, so keep it for real alerts')
        for p in Path(dir_).iterdir():
            if p.suffix.lower() in (".json", ".db", ".sqlite", ".txt") and p.stat().st_mtime > time.time() - 30 \
                    and p.name not in ("package.json", "package-lock.json", "requirements.txt"):
                r.warn(f"{p.name} in the plugin folder changed during the check. Save data in NOOK_DATA_DIR: "
                       f"updates replace the plugin folder")

    print(f"Nook plugin check: {dir_}")
    if m:
        print(f"  {m.get('name')} ({m.get('id')}) | {m.get('mode', 'live')} | {m.get('size', 'small')} | run: {' '.join(m.get('run', []))}")
    if stats["types"]:
        sent = ", ".join(f"{k} x{v}" for k, v in sorted(stats["types"].items()))
        print(f"  sent: {sent}")
    if stats["ids"]:
        print(f"  control ids: {', '.join(sorted(stats['ids']))}")
    if stderr_tail:
        print("  stderr (last lines):")
        for l in stderr_tail[-6:]:
            print(f"    {l}")
    for title, items in (("ERRORS", r.errors), ("WARNINGS", r.warnings), ("NOTES", r.notes)):
        if items:
            print(f"\n{title} ({len(items)})")
            for i in items:
                print(f"  - {i}")
    print("\nOK: no errors" if not r.errors else f"\nFAILED: {len(r.errors)} error(s)")
    sys.exit(1 if r.errors else 0)


if __name__ == "__main__":
    main()
