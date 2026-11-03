"""Windows side of Key Remap: registers global hotkeys on a thread of its
own and, when one is pressed, types the original shortcut with SendInput."""

import ctypes
import threading
import time
from ctypes import wintypes

import remap

user32 = ctypes.windll.user32
kernel32 = ctypes.windll.kernel32
WM_HOTKEY, WM_APP = 0x0312, 0x8000
MOD_NOREPEAT = 0x4000
KEYEVENTF_KEYUP = 0x0002
ULONG_PTR = ctypes.c_size_t


class MOUSEINPUT(ctypes.Structure):
    _fields_ = [("dx", wintypes.LONG), ("dy", wintypes.LONG), ("mouseData", wintypes.DWORD),
                ("dwFlags", wintypes.DWORD), ("time", wintypes.DWORD), ("dwExtraInfo", ULONG_PTR)]


class KEYBDINPUT(ctypes.Structure):
    _fields_ = [("wVk", wintypes.WORD), ("wScan", wintypes.WORD), ("dwFlags", wintypes.DWORD),
                ("time", wintypes.DWORD), ("dwExtraInfo", ULONG_PTR)]


class HARDWAREINPUT(ctypes.Structure):
    _fields_ = [("uMsg", wintypes.DWORD), ("wParamL", wintypes.WORD), ("wParamH", wintypes.WORD)]


class _INPUT_UNION(ctypes.Union):
    _fields_ = [("mi", MOUSEINPUT), ("ki", KEYBDINPUT), ("hi", HARDWAREINPUT)]


class INPUT(ctypes.Structure):
    _fields_ = [("type", wintypes.DWORD), ("u", _INPUT_UNION)]


def _held_mask():
    mask = 0
    for m, vk in remap.MOD_VKS:
        if user32.GetAsyncKeyState(vk) & 0x8000:
            mask |= m
    return mask


def press(target):
    """Types `target` (e.g. 'ctrl+alt+KeyQ') as if the user pressed it."""
    events = remap.plan(_held_mask(), target)
    arr = (INPUT * len(events))()
    for i, (vk, down) in enumerate(events):
        arr[i].type = 1  # INPUT_KEYBOARD
        arr[i].u.ki = KEYBDINPUT(vk, 0, 0 if down else KEYEVENTF_KEYUP, 0, 0)
    user32.SendInput(len(events), arr, ctypes.sizeof(INPUT))


class Engine:
    """set(mappings) with mappings = [(key, new, old)]; on_result(errors)
    gets {key: reason or None} after every (re)registration."""

    def __init__(self, on_result):
        self.on_result = on_result
        self._pending = []
        self._targets = {}  # hotkey id -> original shortcut
        self._ids = []
        self._ready = threading.Event()
        self._tid = 0
        threading.Thread(target=self._run, daemon=True).start()
        self._ready.wait(5)

    def set(self, mappings):
        self._pending = list(mappings)
        user32.PostThreadMessageW(self._tid, WM_APP, 0, 0)

    def stop(self):
        user32.PostThreadMessageW(self._tid, 0x0012, 0, 0)  # WM_QUIT

    def _run(self):
        msg = wintypes.MSG()
        user32.PeekMessageW(ctypes.byref(msg), None, 0, 0, 0)  # creates the queue
        self._tid = kernel32.GetCurrentThreadId()
        self._ready.set()
        while user32.GetMessageW(ctypes.byref(msg), None, 0, 0) > 0:
            if msg.message == WM_APP:
                self._reload()
            elif msg.message == WM_HOTKEY:
                target = self._targets.get(msg.wParam)
                if target:
                    time.sleep(0.03)
                    press(target)
        for i in self._ids:
            user32.UnregisterHotKey(None, i)

    def _reload(self):
        for i in self._ids:
            user32.UnregisterHotKey(None, i)
        self._ids, self._targets, result = [], {}, {}
        for n, (key, new, old) in enumerate(self._pending, 1):
            try:
                mods, vk = remap.parse(new)
            except ValueError as e:
                result[key] = str(e)
                continue
            if user32.RegisterHotKey(None, n, mods | MOD_NOREPEAT, vk):
                self._ids.append(n)
                self._targets[n] = old
                result[key] = None
            else:
                result[key] = "taken by another program"
        self.on_result(result)
