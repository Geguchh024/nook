"""Pure logic of Key Remap: parsing shortcuts and planning key presses.
No Windows calls here, so it runs (and is tested) anywhere."""

MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN = 0x1, 0x2, 0x4, 0x8
MOD_NAMES = {"alt": MOD_ALT, "ctrl": MOD_CONTROL, "control": MOD_CONTROL,
             "shift": MOD_SHIFT, "win": MOD_WIN, "super": MOD_WIN, "meta": MOD_WIN}
# Virtual-key codes of the modifier keys, in press order.
MOD_VKS = [(MOD_CONTROL, 0x11), (MOD_ALT, 0x12), (MOD_SHIFT, 0x10), (MOD_WIN, 0x5B)]
NAMED = {"space": 0x20, "enter": 0x0D, "return": 0x0D, "escape": 0x1B, "esc": 0x1B,
         "tab": 0x09, "backspace": 0x08, "insert": 0x2D, "delete": 0x2E,
         "home": 0x24, "end": 0x23, "pageup": 0x21, "pagedown": 0x22,
         "left": 0x25, "up": 0x26, "right": 0x27, "down": 0x28}

# Nook's built-in tools and their shortcuts (Nook 0.2.x). Edit if yours differ.
DEFAULT_TOOLS = [
    ("qr", "QR code", "ctrl+alt+KeyQ"),
    ("screenshot", "Screenshot", "ctrl+alt+KeyS"),
    ("text", "Text extractor", "ctrl+alt+KeyT"),
    ("color", "Color picker", "ctrl+alt+KeyP"),
    ("lens", "Lens search", "ctrl+alt+KeyL"),
    ("calc", "Calculator", "ctrl+alt+KeyK"),
    ("draw", "Draw", "ctrl+alt+KeyD"),
    ("voice", "Voice memo", "ctrl+alt+KeyR"),
    ("font", "Font detector", "ctrl+alt+KeyF"),
    ("coffee", "Coffee", "ctrl+alt+KeyC"),
]

# Shortcuts that are active from the first start (entry id -> new shortcut).
DEFAULT_NEW = {"qr": "ctrl+shift+KeyQ"}


def parse(text):
    """'ctrl+shift+Q' or 'ctrl+alt+KeyQ' -> (modifier_mask, virtual_key).
    Needs at least one modifier. Raises ValueError with a readable reason."""
    parts = [p.strip() for p in str(text).split("+") if p.strip()]
    if len(parts) < 2:
        raise ValueError("need a modifier and a key, e.g. ctrl+shift+Q")
    mods = 0
    for p in parts[:-1]:
        m = MOD_NAMES.get(p.lower())
        if m is None:
            raise ValueError(f"unknown modifier '{p}'")
        mods |= m
    key = parts[-1]
    low = key.lower()
    if low.startswith("key") and len(low) == 4 and low[3].isalpha():
        low = low[3]
    elif low.startswith("digit") and len(low) == 6 and low[5].isdigit():
        low = low[5]
    if len(low) == 1 and low.isalpha():
        return mods, ord(low.upper())
    if len(low) == 1 and low.isdigit():
        return mods, ord(low)
    if low in NAMED:
        return mods, NAMED[low]
    if low.startswith("f") and low[1:].isdigit() and 1 <= int(low[1:]) <= 24:
        return mods, 0x70 + int(low[1:]) - 1
    raise ValueError(f"unknown key '{key}'")


def pretty(text):
    """Display form: 'Ctrl+Shift+Q'."""
    mods, vk = parse(text)
    names = [n for m, n in ((MOD_CONTROL, "Ctrl"), (MOD_ALT, "Alt"),
                            (MOD_SHIFT, "Shift"), (MOD_WIN, "Win")) if mods & m]
    if 0x41 <= vk <= 0x5A or 0x30 <= vk <= 0x39:
        key = chr(vk)
    elif 0x70 <= vk <= 0x87:
        key = f"F{vk - 0x6F}"
    else:
        key = next(n for n, v in NAMED.items() if v == vk).capitalize()
    return "+".join(names + [key])


def plan(held_mask, target):
    """Key events (vk, is_down) that make Windows see `target` pressed,
    even though the user still holds the keys of the new shortcut."""
    mods, vk = parse(target)
    events = [(v, False) for m, v in MOD_VKS if held_mask & m]
    events += [(v, True) for m, v in MOD_VKS if mods & m]
    events += [(vk, True), (vk, False)]
    events += [(v, False) for m, v in reversed(MOD_VKS) if mods & m]
    return events


def check(entries, key, new):
    """Reason `new` can't be used for entry `key`, or None if fine."""
    mods, vk = parse(new)
    for e in entries:
        if e["id"] == key:
            if parse(e["old"]) == (mods, vk):
                return "same as the original shortcut"
        elif e.get("new") and parse(e["new"]) == (mods, vk):
            return f"already used for {e['name']}"
    return None
