# Nook plugin protocol (version 1)

Source of truth: `crates/modules/src/plugins/protocol.rs`, `host.rs` and `manifest.rs` in the Nook repository.

## Contents
1. Transport
2. plugin.toml
3. Environment variables
4. Plugin → Nook (stdout)
5. Nook → plugin (stdin)
6. Event payloads by control
7. Live mode lifecycle
8. Poll mode lifecycle
9. Discovery, dev mode and installs

## 1. Transport

- **stdout**: plugin → Nook. One JSON object per line, UTF-8, no newlines inside an object. Flush after every line.
- **stdin**: Nook → plugin (live mode only). One JSON object per line.
- **stderr**: free text. Goes to `%APPDATA%\Nook\nook.log` (tagged with the plugin id), and the last 6 lines are shown when the plugin stops.
- A line that is not JSON, or a message missing a required field, is skipped and logged as `bad "<type>" view: <serde error>` or `not JSON: ...`. Nothing else happens, so a skipped first `tile` just leaves the default tile (icon + name).
- A line whose `type` is not a message type (below) is parsed as a **view node** and used as the tile: printing `{"type":"text","text":"Hi"}` equals `{"type":"tile","view":{"type":"text","text":"Hi"}}`.
- A message without `type` is an error.

## 2. plugin.toml

Parsed with unknown keys **denied**: any key not listed here makes the manifest invalid and the plugin is skipped (logged as `plugin <path>: unknown field ...`).

| Key | Type | Default | Rule |
|---|---|---|---|
| `id` | string | required | 1–48 chars, only `a-z 0-9 - _`. Module id becomes `plugin:<id>`. Same id in two folders: the first folder searched wins. |
| `name` | string | required | Page title and tile editor label. |
| `run` | array of strings | required | Non-empty; first item non-blank. Runs with the plugin folder as working directory. `run[0]` containing `/` or `\`, or naming a file that exists in the folder, is resolved in the folder; otherwise it is looked up on `PATH`. |
| `icon` | string | `"puzzle-piece"` | Phosphor name (see `icons.txt`). Unknown → `?`. |
| `size` | `"small"`, `"wide"`, `"large"` | `"small"` | Starting tile size; the user can change it. |
| `mode` | `"live"`, `"poll"` | `"live"` | |
| `interval` | number (seconds) | `60` | Poll mode only; must be ≥ 1. |
| `hotkey` | string | none | `global-hotkey` syntax, e.g. `"ctrl+alt+KeyW"`. Sends `hotkey`. |
| `accepts_files` | bool | `false` | Files dropped on the notch go to this plugin while its page is open. |
| `page_size` | `[width, height]` points | panel size | Panel grows to this while the page is open (kept inside the monitor). |
| `description`, `author`, `version` | string | `""` | Information only (logs, marketplace). |

## 3. Environment variables

Set for every run (live process or each poll run):

| Variable | Value |
|---|---|
| `NOOK_PLUGIN_ID` | the manifest `id` |
| `NOOK_PLUGIN_DIR` | the plugin folder (also the working directory) |
| `NOOK_DATA_DIR` | `%APPDATA%\Nook\plugin-data\<id>`, created by Nook. Persist everything here. |
| `NOOK_PROTOCOL` | `1` |
| `NOOK_VERSION` | Nook's version, e.g. `0.1.0` |
| `NOOK_MODE` | `live` or `poll` |
| `NOOK_THEME` | `dark` or `light` |
| `NOOK_ACCENT` | accent color `#RRGGBB` |
| `NOOK_EVENT` | poll mode only, when a run was caused by an event: that message as JSON |
| `PYTHONUNBUFFERED`, `PYTHONIOENCODING` | `1`, `utf-8` |

The process gets no console window (`CREATE_NO_WINDOW`).

## 4. Plugin → Nook (stdout)

| `type` | Fields | Effect |
|---|---|---|
| `tile` | `view` (node, required) | Replaces the tile content. |
| `page` | `view` (node, required), `actions` (optional array) | Replaces the page. Once a page has been sent, clicking the tile (outside your controls) opens it. |
| `badge` | `badge` (object or `null`, required) | What the collapsed pill shows for you; `null` clears it. |
| `active` | `active` (bool, required) | Lights the tile with the accent gradient. |
| `open_page` | | Opens your page (if the panel is open). |
| `close_page` | | Closes your page. |
| `show` | | Opens the panel, pinned, on your page. Use for alerts, sparingly. |
| `copy` | `text` (string, required) | Puts text on the clipboard. |
| `open` | `target` (string, required) | Opens a URL, file or folder with the default app. |
| `log` | `message` (required), `level` (`debug`/`info`/`warn`/`error`, default info) | Writes to Nook's log. |

**Page actions**: `[{ "id": "refresh", "icon": "arrow-clockwise", "tooltip": "Refresh" }]`. `id` and `icon` required. Round buttons in the page header, laid out right to left; a click sends `event` with that `id`, `kind: "click"`.

**Badges** (`kind` is the tag):

| Badge | Fields | Shows |
|---|---|---|
| `{"kind":"ring","progress":0.4,"label":"12:00"}` | `progress` 0..1 required, `label` optional | Ring + label replacing the pill's clock (only the first ring among all modules shows). |
| `{"kind":"glyph","icon":"bell"}` | `icon` required | Small filled icon on the right of the pill. |
| `{"kind":"pulse"}` | | Pulsing accent dot: something arrived. |

## 5. Nook → plugin (stdin, live mode)

| `type` | Fields | When |
|---|---|---|
| `hello` | `protocol`, `nook_version`, `plugin_id`, `data_dir`, `theme: {dark, accent}` | First message after start. |
| `layout` | `target` (`tile`/`page`), `size` (`small`/`wide`/`large` for tiles, `page` for pages), `width`, `height` (points) | When the tile or page is first drawn and whenever its size changes. Re-sent after a restart. |
| `theme` | `dark`, `accent` | The user changed theme or accent. |
| `event` | `id`, `kind`, `value` (optional) | A control or page action was used. See §6. |
| `tile_click` | | The tile was clicked outside your controls (if you have a page, it also opens). |
| `page_open` / `page_close` | | Your page appeared / has not been drawn for ~300 ms. |
| `hotkey` | | The manifest `hotkey` was pressed. |
| `files` | `paths` (array of strings) | Files dropped while your page is open (`accepts_files = true`). |
| `shutdown` | | Nook is quitting, reloading or replacing the plugin. Save now; the process is killed 0.4 s later. |

Ignore unknown `type`s and unknown event ids: future versions add more.

## 6. Event payloads by control

`{"type":"event","id":"<control id>","kind":"<kind>","value":<value>}`

| Source | `kind` | `value` |
|---|---|---|
| `button`, page action | `click` | absent |
| `list` item | `click` | absent; `id` is the **item's** id |
| `toggle` | `change` | `true` / `false` (new state) |
| `slider` | `change` while dragging, `commit` on release | number (snapped to `step`, clamped to min..max) |
| `input` | `submit` on Enter; also `change` on every edit if `live: true` | string (current text) |
| `chips` | `change` | `{"index": 2, "label": "Week"}` |
| `canvas` with an `id` | `click` | `{"x": 31, "y": 12}` in canvas points from its top left |

## 7. Live mode lifecycle

1. Nook starts `run` in the plugin folder and sends `hello`.
2. You print `tile` (and `page`, `badge` ... as needed) whenever state changes.
3. Nook sends `layout`, `event`, `theme`, `page_open/close`, `hotkey`, `files` as they happen.
4. On quit/reload/update Nook sends `shutdown`, closes stdin, waits 0.4 s, then kills the process.
5. If the process exits on its own, Nook restarts it after 2 s, up to 3 times within 60 s; after that the tile shows *Stopped · click to restart* with the error and last stderr lines on hover. A user click restarts it.

stdin closing (EOF) means Nook is gone: exit.

## 8. Poll mode lifecycle

1. Nook runs `run` to completion with stdin closed; you print a tree and exit.
2. Output may be **one JSON document** (pretty-printed is fine) or **one message per line**.
3. Nook waits `interval` seconds, or until an event arrives (click, submit, `tile_click`, `hotkey`, `page_open`...), then runs again with that message in `NOOK_EVENT`.
4. A run that takes over **30 s** is killed and counts as failed. A non-zero exit with empty stdout shows the last stderr line as the error. Poll plugins are never marked Stopped; they simply run again next time.
5. `theme`, `layout` and `hello` do not trigger a run; they only change the environment of the next one.

PowerShell output: `ConvertTo-Json -Depth 20 -Compress` (default depth 2 flattens nested views), `[Console]::OutputEncoding = [Text.Encoding]::UTF8`, wrap child lists in `@(...)`.

## 9. Discovery, dev mode and installs

- Search order: `%APPDATA%\Nook\plugins\*`, `plugins\*` next to `nook.exe`, then every folder in `NOOK_PLUGIN_PATH` (`;`-separated). Each subfolder with a `plugin.toml` is a plugin. Hidden folders (starting with `.`) are skipped.
- **Dev mode** (Settings → *Plugin dev mode*, or `NOOK_PLUGIN_DEV=1`, or `--plugin-dev`): saving any file in a plugin folder (two levels deep; `node_modules`, `__pycache__` and dotfiles ignored) restarts the plugin and re-reads `plugin.toml`; new plugin folders load and removed ones unload within a second.
- **Installs from the website** (`nook://install?...`) unpack into `%APPDATA%\Nook\plugins\<id>`, replacing any existing folder with that id. `plugin.toml`'s id must equal the listing's id. Data in `NOOK_DATA_DIR` is kept.
- Flags for screenshots and testing: `--pinned` (start open), `--page=plugin:<id>` (open on your page).
