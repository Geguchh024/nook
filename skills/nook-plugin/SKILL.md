---
name: nook-plugin
description: Build, fix, test and publish plugins (tiles) for Nook, the floating notch for Windows. A Nook plugin is a folder with a plugin.toml and a program in any language that prints JSON views on stdout and reads events on stdin. Use this skill whenever the user wants to make, change, debug or publish a Nook plugin, module, tile or widget for "the notch", mentions plugin.toml, NOOK_DATA_DIR or other NOOK_ environment variables, nook.py or nook.js, %APPDATA%\Nook\plugins, the Nook marketplace, or a tile that "doesn't show up" or "says Stopped", even if they never say the word "plugin".
---

# Nook plugins

Nook is a notch-style panel at the top of the Windows screen. Hovering over it opens a grid of tiles. Anyone can add a tile without touching Nook's code: a **plugin** is a folder with a `plugin.toml` and a program. Nook starts the program and the two exchange **one JSON object per line**: the program prints *views* (trees of components such as `row`, `text`, `button`, `ring`) on stdout, and Nook draws them in its own style and sends back *events* (clicks, edits, hotkeys, dropped files) on stdin.

Getting a plugin right is mostly about honoring that contract exactly. Nook skips any line it can't parse (and only logs why), so small mistakes show up as a blank tile rather than an error. This skill gives you the exact contract, a checker that runs the plugin the way Nook does, and the rules that prevent the common failures.

## Files in this skill

| File | Read it when |
|---|---|
| `references/protocol.md` | Writing the message loop: every message in both directions, event payloads, environment variables, live vs poll lifecycle. |
| `references/components.md` | Building views: every component and canvas shape with required fields, defaults and the events it sends; colors, text sizes, tile sizes. |
| `references/icons.txt` | Picking icons. The only valid icon names (Phosphor, kebab-case), one per line. Search it, don't guess. |
| `assets/nook.py`, `assets/nook.js` | Python 3.8+ and Node 18+ helpers, one dependency-free file each. Copy the one you need next to the plugin's main file. |
| `scripts/check_plugin.py` | After every change. Validates `plugin.toml`, runs the plugin like Nook does, and checks everything it prints. |

## Workflow

### 1. Decide mode, size and language

**Mode.** `live` (default) keeps one process running and talks over stdin/stdout. Use it for anything with state, timers, instant reactions or animation. `poll` runs the program to completion every `interval` seconds and again on each click, passing the triggering event in `NOOK_EVENT`. Use it for "fetch something and show it" plugins, especially in PowerShell, which needs nothing installed.

**Size.** Tiles are small: content area is 44×44 points (`small`), 104×36 (`wide`) or 104×104 (`large`). The user can resize them, so read the `layout` message. Put the glanceable thing on the tile and everything else on a **page** (536×168 by default, or `page_size` in the manifest), which opens when the tile is clicked.

**Language.** Use what the user already has. Python or Node: copy `assets/nook.py` or `assets/nook.js` into the plugin folder and use it, since it handles the loop, flushing, data storage and error logging. PowerShell: poll mode with plain `ConvertTo-Json`. Anything compiled: ship the binary in the folder and set `run = ["bin/name.exe"]`.

### 2. Lay out the folder

```
my-plugin/
  plugin.toml
  main.py          # or main.js, script.ps1, bin/tool.exe
  nook.py          # copied from assets/, if Python
```

To see it running, the folder must be somewhere Nook looks: `%APPDATA%\Nook\plugins\<folder>` or a folder listed in `NOOK_PLUGIN_PATH` (`;`-separated, each entry holding plugin folders). Developing in a repo elsewhere is fine: point `NOOK_PLUGIN_PATH` at the folder *above* the plugin.

### 3. Write plugin.toml

```toml
id      = "weather"            # 1-48 chars of a-z 0-9 - _ ; never change it later
name    = "Weather"
icon    = "cloud-sun"          # must be in references/icons.txt
size    = "wide"               # small | wide | large
run     = ["python", "main.py"]
# mode = "poll"  interval = 300            (poll plugins only)
# hotkey = "ctrl+alt+KeyW"                 (optional)
# accepts_files = true                     (files dropped on the open page)
# page_size = [568, 300]                   (taller/wider page, in points)
description = "Current weather for your city."
author  = "you"
version = "1.0.0"
```

Only these keys exist: `id name icon size run mode interval hotkey accepts_files page_size description author version`. **Any other key makes the whole manifest invalid and the plugin silently does not load.** Put settings in the program or the data folder, not in `plugin.toml`.

### 4. Write the program around one `draw()`

Keep all state in variables, write one function that builds the *whole* tree from that state and sends it, and call it after every change. Nook replaces the previous tree with each one you send, so there is nothing to diff or patch.

```python
import nook

app = nook.Plugin()
count = app.load("count", 0)                  # JSON file in NOOK_DATA_DIR

def draw():
    app.tile(nook.column([
        nook.text(count, size="title"),
        nook.row([nook.button("dec", icon="minus"),
                  nook.button("inc", icon="plus", primary=True)], gap=2),
    ], gap=0, align="center", justify="center"))

def change(by):
    global count
    count += by
    app.save("count", count)
    draw()

app.on("inc")(lambda e: change(1))
app.on("dec")(lambda e: change(-1))
draw()
app.run()
```

Node is the same shape (`app.setTile`, `app.on`, `nook.row([...], {gap: 2})`; see the top of `assets/nook.js`). A PowerShell poll plugin just prints one tree:

```powershell
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$b = Get-CimInstance Win32_Battery
@{ type = 'tile'; view = @{ type = 'ring'; value = $b.EstimatedChargeRemaining / 100
   label = "$($b.EstimatedChargeRemaining)%"; tooltip = 'Battery' } } |
  ConvertTo-Json -Depth 20 -Compress
```

### 5. Check it

```bash
python scripts/check_plugin.py path/to/my-plugin
python scripts/check_plugin.py path/to/my-plugin --event inc:click --event city:submit:Paris
```

The checker validates the manifest, confirms `run` resolves, starts the plugin with the same environment and messages Nook sends (`hello`, `layout`, then your `--event`s, `page_open` and `shutdown`), and checks every line printed against the protocol, including component fields, colors and icon names. Fix every **error**; read every **warning**, since they usually point to a tile that looks wrong. Run it again after each change. It is much faster than restarting Nook and reading logs.

### 6. Try it in Nook

In Nook: **Settings → Plugin dev mode** on. With dev mode on, a new plugin folder loads within a second, saving any file restarts the plugin and re-reads `plugin.toml`, and no restart of Nook is needed. (`NOOK_PLUGIN_DEV=1` does the same for one run.) If the tile says **Stopped · click to restart**, hover over it for the error and the last stderr lines. Everything else is in `%APPDATA%\Nook\nook.log`, where each skipped line is logged with the reason, e.g. `bad "button" view: missing field id`.

### 7. Publish

Push the folder to a **public GitHub repository** (the plugin can sit at the root or in a subfolder), then submit the repository link on the Nook website's *Submit a plugin* page. The site re-checks `plugin.toml` with Nook's own rules. After review, the listing gets an **Install in Nook** button that installs the exact commit that was reviewed, live, with no restart. Later pushes reach users only when the listing is updated, so bump `version` when you change things.

## Rules that prevent most bugs

**stdout carries protocol only.** Each line must be one complete JSON object, UTF-8, flushed right away. A stray `print("debug")` becomes a skipped "bad line", and in poll mode it can spoil the whole output. Send debugging to **stderr** (`print(..., file=sys.stderr)`, `console.error`), which goes to Nook's log and is shown on a stopped tile. The helpers flush for you. In Go, Rust, C or C#, flush after every line yourself.

**Keep data in `NOOK_DATA_DIR`, never in the plugin folder.** Installing an update replaces the plugin folder wholesale, so anything saved next to the code is lost. `NOOK_DATA_DIR` (`%APPDATA%\Nook\plugin-data\<id>`) survives updates and removals. `app.load` and `app.save` already use it.

**Control ids are the API between your view and your handlers.** Every `button`, `toggle`, `slider`, `input`, `chips` and `list` item needs an `id`, and that id must stay the same across redraws. Give list items and per-row buttons ids that encode the row, like `"task:42"` or `"drive:C:"`, and route them in a catch-all handler (`@app.on_message("event")` in Python, `app.onMessage("event", fn)` in Node).

**Controls keep the user's value until you send a *different* one.** A toggle flips and an input keeps typed text before your program answers. Nook compares each `value` with the last value *you* sent for that id, not with what is on screen: sending the same value again changes nothing, and sending a new one replaces whatever the user did. Two consequences:
- Echo back the state you actually hold (`toggle(id, value=self.on)`), so a redraw never reverts the user.
- To **clear an input after `submit`**, resending `value: ""` does nothing if you sent `""` before. Give the field a fresh id instead, e.g. `f"new:{n}"` with `n` bumped on each submit, and match the prefix in the catch-all handler. (Mirroring every keystroke with `live: true` and echoing it back also works, but a slow round trip can drop keystrokes.)

**Fit the tile.** A small tile holds one icon, one ring or a short number. Text that doesn't fit is cut with "…" unless `wrap: true`. Use `layout` (`app.tile_size` / `app.tile.size` in the helpers) to show more when the user makes the tile wide or large, and move lists, inputs and settings to the page.

**Use theme colors.** `text`, `dim`, `faint`, `accent`, `on_accent` follow the user's light/dark theme and accent, so the tile looks native in both. Use fixed colors (`red green orange ...`) only for meaning (an alert, a status). Hex works but won't adapt.

**Be silent when nothing changes.** Nook idles at 0% CPU, and a plugin that redraws on a fast timer costs CPU and battery for nothing. Redraw on events and on real changes; for clocks, redraw once per second or minute, and for expensive work (network, disk scans), start it on `page_open` and stop it on `page_close`. Animate (about 30 updates a second) only while something actually moves.

**Live plugins must not exit.** If the process ends, Nook restarts it after 2 s, at most 3 times a minute, then shows *Stopped*. Catch exceptions inside handlers (the helpers already do and log them as errors). On `shutdown`, save state immediately: the process is killed 0.4 s later.

**Poll plugins must finish in 30 seconds** and may print either one JSON document (pretty-printing is fine) or one message per line. In PowerShell always use `ConvertTo-Json -Depth 20 -Compress`: the default depth of 2 silently flattens nested views into strings. Wrap arrays in `@(...)` so one-element lists stay lists, and set `[Console]::OutputEncoding` to UTF-8.

**Icons must come from `references/icons.txt`.** An unknown name renders as a question mark. Names are kebab-case Phosphor names (`cloud-sun`, `hard-drives`, `arrow-clockwise`).

**Hotkeys** use `global-hotkey` syntax: modifiers then one key, e.g. `"ctrl+alt+KeyW"`, `"ctrl+shift+Digit1"`, `"alt+F5"`. Nook already uses Ctrl+Alt with C, D, F, K, L, N, P, Q, R, S and T, and the to-do example uses Ctrl+Alt+Y, so pick something else.

**Ignore what you don't know.** Skip unknown message types and unknown event ids without crashing: newer versions of Nook add more.

**Plugins run with the user's rights.** Don't hard-code API keys or tokens in the code. Ask for them on the page with an `input` and store them in the data folder. Only open URLs and run commands the user asked for. Reviewers read what a plugin runs and what it sends where, so keep network calls obvious and few.

## When something doesn't show up

| Symptom | Usual cause |
|---|---|
| Plugin not in the grid at all | Manifest invalid (unknown key, bad `id`, empty `run`), folder not in a searched location, or the same `id` already loaded from another folder (the first one found wins). Run the checker; read `nook.log`. |
| Tile shows only the icon and name | The program never printed a valid `tile`. Usually stdout is buffered (no flush), the JSON is invalid, or the first message failed validation. |
| *Stopped · click to restart* | The program exited or couldn't start (`run[0]` not on PATH, such as `python` vs `py`, or a missing module). Hover the tile for stderr. |
| Clicks do nothing | The handler's id doesn't match the control's `id`, or a poll plugin ignores `NOOK_EVENT`. |
| An input won't clear, or a control jumps back | Nook only applies a `value` that differs from the last one you sent (see "Controls keep the user's value"). Bump the input's id to clear it; echo real state for toggles. |
| `?` instead of an icon | Icon name not in `references/icons.txt`. |
