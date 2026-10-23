# Nook

A floating notch for Windows 10/11. A small glass pill sits at the top
of the screen (center, or either corner). Hover it and it springs open into a bento grid of
utilities. Move away and it folds back. It idles at 0% CPU.

Built in Rust with eframe/egui (OpenGL), the `windows` crate (Win32, DWM,
Windows.UI.Composition, Windows.Media.Ocr), `tray-icon` and `global-hotkey`.
No network calls and no telemetry: everything stays on your machine. The
exceptions happen only when you use them: Lens search (your browser sends
the area you selected to Google Lens), and installing a plugin from the
website (Nook downloads it from GitHub).

## Build and run

Requirements:

- Rust stable, **MSVC** toolchain. `rust-toolchain.toml` pins
  `stable-x86_64-pc-windows-msvc`, and rustup installs it on first build.
- Visual Studio 2022 Build Tools with the **Desktop development with C++**
  workload (MSVC v143 and a Windows 11 SDK):

  ```bash
  winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
  ```

Then run:

```bash
cargo run --release
```

Other useful commands:

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo run --release --features nook-app/debug-overlay
```

The last one adds a small overlay showing the frame count, CPU time per
frame, the interval between frames and why egui repainted.

Dev flags: `--pinned` starts expanded and pinned. `--settings` opens the
settings page, `--page=<module id>` a module's page (e.g. `--page=phone_link`).

### Using it

- **Hover** the pill for 120 ms to expand. **Leave** for 250 ms to
  collapse.
- **Click** the glass, press **Ctrl+Alt+N**, or use the tray's
  **Pin open** to keep the panel open. A small pin shows at the top center.
- **Position:** Settings, Position: Left, Center or Right. In a corner
  the panel opens toward the other side.
- **Always on top:** on by default. Turn it off in Settings or the tray
  to keep Nook behind your windows (it still shows on the desktop).
  Ctrl+Alt+N brings it back on top.
- **Tray menu:** Settings, Pin open, Always on top, Launch at startup, Quit.
- **Settings** (gear tile): theme (Auto/Dark/Light), accent
  (Wallpaper/Windows/Violet), material (Glass/Solid), position
  (Left/Center/Right), monitor (Primary/Follow cursor), hover delay,
  always on top, launch at startup.
- **Edit the layout:** the pencil at the bottom left of the grid. The grid
  becomes a live preview of itself: drag a tile and the others spring out
  of its way as it repacks (rest on a page dot to carry it to another
  page). Each tile shows its icon, a tick under it to show or hide it
  (hidden tiles stay dimmed in edit mode), and Small (1x1), Wide (2x1) or
  Large (2x2) to resize it. The check where the pencil was finishes.
- **Compact tiles open in place:** a tile made smaller than its layout
  needs (timer, media, clipboard, coffee, notes) shows its icon, name and
  live status. Click it and the grid repacks with it at full size: it
  grows in place while the other tiles glide out of its way, and they stay
  usable. Click anywhere else (or press Esc) to fold it back.
- **Wallpaper:** the tile shows your current wallpaper. Click it, choose a
  folder (subfolders two levels down are included), and its pictures line
  up in a filmstrip under a large preview. Scroll the strip sideways with
  the wheel, a trackpad or a drag. Hover a picture to preview it, and click
  it to set it on every monitor. The accent color follows the new wallpaper.
- Nook hides itself while a fullscreen app or game is in front.
- **Screen tools** work on every connected monitor. They freeze all of
  them first, so menus and video hold still, and a selection can span
  monitors. Hints and the loupe follow the cursor at each monitor's
  scaling. Esc or right click cancels.
  - **Text extractor** (Ctrl+Alt+T): drag over text and it is copied, one
    line per row. Hold Shift as you let go to get a single line. Uses the
    OCR built into Windows, for the languages whose OCR packs are installed.
  - **Color picker** (Ctrl+Alt+P): a loupe follows the cursor. Scroll to
    zoom, arrows nudge by a pixel (Shift: ten), click or Enter copies the
    color. Right click the tile for the history and the copy format (HEX,
    RGB, HSL, HSV, OKLCH).
  - **Lens search** (Ctrl+Alt+L): drag over anything and Google Lens opens
    in your default browser with the results.
  - **Font detector** (Ctrl+Alt+F): drag over a line of text. The text is
    redrawn in every installed font and compared pixel by pixel. If the
    [Lens](https://github.com/mixfont/lens) model by Mixfont is present, its
    neural network also names open-source fonts (983 families, mostly Google
    Fonts) that are not installed. It runs on the device with no network.
    The model is not bundled, because Lens is licensed for personal and
    non-commercial use only. To enable it, copy `font_classifier.pt` and
    `classes.json` from Lens's `model/` folder to `%APPDATA%\Nook\fontnet\`
    (or `fontnet\` next to `nook.exe`, or `assets\fontnet\` in a checkout).

- **Phone link** (the phone tile): your Android phone inside the notch.
  The page grows the panel to fit a phone screen.
  - **Files and text, both ways**, over the local network with the
    [LocalSend](https://localsend.org) protocol, implemented in Rust (no
    cloud, no account). The phone only needs the LocalSend app open. Drop
    files or folders on the open page, or use Choose files; Send clipboard
    sends your copied text as a message. Incoming files ask first (the
    notch opens on the request) unless auto-accept is on, and land in
    `Downloads\Nook`. The pill shows a progress ring while a transfer runs.
    Allow Nook through the Windows firewall when asked, or nothing can
    reach it.
  - **Screen mirroring** with [scrcpy](https://github.com/Genymobile/scrcpy)
    (Install with WinGet from the page if it is missing). The mirror sits
    in the panel with rounded corners. Click it, or the cursor button, to
    control the phone with your mouse and keyboard; **Esc** gives them back.
    Clipboard syncs both ways while mirroring; phone audio and turning the
    phone's screen off are options in Setup.
  - **Notifications** over adb: the Alerts tab lists them, the pill pulses
    when new ones arrive, and clicking one opens its app on the phone and
    mirrors it.
  - Back, home, recents, volume and power keys under the phone.
  - **USB or Wi-Fi:** turn on USB debugging and plug in, then Setup can
    switch the phone to Wi-Fi in one click (`adb tcpip`), connect to an
    address, or pair with an Android 11+ wireless debugging code. The last
    Wi-Fi address reconnects at start-up. Settings are in
    `%APPDATA%\Nook\phone.txt`.

- **Tools** (second page of the grid). Each opens a full page; made Wide
  or Large, most show more on the tile itself.
  - **QR code** (Ctrl+Alt+Q makes one from the clipboard): paste a link and
    the code draws as you type. Styles: dots, liquid, round lines, rounded,
    square; corner eyes round, circle, leaf or square; solid colors,
    gradients or the accent; white, cream, night or clear background; your
    logo in the middle (drop an image on the notch, error correction rises
    to High) and a label under it. Copy, save a 1200 px PNG or a vector
    SVG to `Pictures\QR codes`, or drag the preview into any app.
  - **Calculator** (Ctrl+Alt+K): answers as you type. `200 + 15%`,
    `20% of 150`, `2π`, `5!`, `17 mod 5`, `0xFF`, functions, DEG/RAD, `ans`,
    and unit conversion (`5 km to mi`, `72 f in c`, `2 GB in MB`,
    `90 min to h`). Whole answers show in hex and binary. Enter keeps the
    answer in the history.
  - **System monitor**: CPU and memory rings on the tile; the page charts
    the last minute of CPU, memory and network, shows free space per drive
    and the busiest apps (by CPU or memory). It samples only while on
    screen.
  - **Calendar**: today's date on the tile, a month view with ISO week
    numbers (the week starts on your region's first day). Type
    `14:30 Dentist` to add an event; timed events chime, open the notch and
    pulse the pill. Saved in `%APPDATA%\Nook\calendar.txt`.
  - **Quick launch**: apps, folders, files, websites and `ms-settings:`
    pages with their Explorer icons. Drop files on the open page, paste a
    path or URL, or browse; the pencil reorders and removes.
  - **Voice memo** (Ctrl+Alt+R from anywhere): records the microphone to
    WAV in `Music\Voice memos` with a live waveform and the time in the
    pill. Play, rename, drag into a chat, show in Explorer or delete.

## Plugins: build your own tiles

Anyone can add a tile to the notch without touching Nook's code or knowing
Rust. A plugin is a folder with a `plugin.toml` and a program in any
language (Python, Node, PowerShell, Go, C#, Rust, ...). The program prints
JSON describing its tile and page, Nook draws it in its own glass style, and
sends back clicks, text input, hotkeys and dropped files. Plugins can show a
badge in the collapsed pill, open the notch, copy to the clipboard and open
links, and the canvas component lets them draw anything.

```toml
# %APPDATA%\Nook\plugins\hello\plugin.toml
id   = "hello"
name = "Hello"
icon = "hand-waving"
size = "wide"
run  = ["python", "main.py"]
```

- **Guide:** [docs/plugins.html](docs/plugins.html): quick start, every
  component and message, live and poll modes, debugging, ideas.
- **Examples:** [examples/plugins](examples/plugins): a counter, a to-do
  list, a PowerShell disk monitor (nothing to install) and an analog clock
  drawn on a canvas.
- **Helpers:** [sdk/python/nook.py](sdk/python/nook.py) and
  [sdk/node/nook.js](sdk/node/nook.js), one dependency-free file each.
- **Claude Code skill:** [skills/nook-plugin](skills/nook-plugin) teaches
  Claude Code the protocol and bundles a checker
  (`skills/nook-plugin/scripts/check_plugin.py <plugin folder>`) that runs a
  plugin the way Nook does and reports what Nook would skip. Install it by
  copying the folder to `%USERPROFILE%\.claude\skills\`; the website's
  docs offer it as a zip. After changing the SDKs or the icon table, run
  `python skills/pack.py` to refresh its copies and the zip.

**Install from the website.** Each plugin's page has an *Install in Nook*
button. It opens a `nook://install?...` link (Nook registers the link type
for the current user every time it starts). If Nook is already running, the
new `nook.exe` hands the link to it over a named pipe and exits. The four
examples are built into `nook.exe` and install at once. Plugins from GitHub
open the notch on the Plugins page, which shows the source and the exact
command the plugin runs; the zip is only downloaded (at the reviewed commit)
once you click Install. Either way the tile appears live, with no restart.
Updates replace the folder in place and keep the plugin's saved data;
Remove sends it to the Recycle Bin.

**Dev mode** (Settings, the Plugins card): saving a file in a plugin's
folder restarts that plugin, and plugin folders that appear or go are loaded
or unloaded within a second, so a new plugin shows up without restarting
Nook. `NOOK_PLUGIN_DEV=1` or `--plugin-dev` turns it on for one run, which
is handy with the examples in a checkout:

```bash
NOOK_PLUGIN_PATH="$PWD/examples/plugins" NOOK_PLUGIN_DEV=1 cargo run --release
```

Plugins run as separate processes. A crash restarts the plugin and never
takes the notch down. They have the same rights as any program you run, so
install only plugins you trust.

Config lives in `%APPDATA%\Nook\config.toml` and logs in
`%APPDATA%\Nook\nook.log`.

## Layout

```
crates/
  app/        eframe entry, window sync (backdrop, hit shape, anchoring, layer), tray, hotkeys,
              config, system watcher, settings page, debug overlay
  ui/         theme tokens, spring animator, squircle geometry,
              shell state machine + renderer (placement), bento grid, widgets, Module trait
  platform/   windows-rs wrappers: window styles, click-through hit testing, z-order,
              Composition backdrop, DWM, monitors, fullscreen detection, power,
              registry, startup, wallpaper
  modules/    one file per utility (timer, media, coffee, clipboard, color picker, draw,
              text extractor, lens, QR code, calculator, calendar, ...), kit.rs for
              the controls the tool pages share, plugins/ for the plugin host
docs/NOTES.md the egui/Windows limitations hit and the workaround for each
docs/plugins.html  the plugin developer guide
examples/plugins/  sample plugins in Python, Node and PowerShell
site/         the website: landing page, plugin marketplace, admin queue and docs
              (Next.js + Convex, see site/README.md)
sdk/          optional one-file plugin helpers for Python and Node
```

Modules implement `nook_ui::module::Module` (`id`, `name`, `icon`,
`size_hint`, `collapsed_badge`, `ui_tile`, `on_tick`, `default_hotkey`, and
a few more) and register in a `Registry`. The bento grid is packed from the
enabled modules in the order stored in config (`[modules] order` and
`disabled`). The trait is object-safe and keyed by string ids, so the plugin
host registers each plugin as a `Box<dyn Module>` (id `plugin:<id>`) and the
shell treats it like any built-in.
Modules do their work off the UI thread and talk to it through channels
drained in `on_tick`.

## Milestone 1 checklist

**Shell, springs, click-through** (done)

- [x] Borderless, transparent, always-on-top tool window, no taskbar entry,
  never steals focus
- [x] Top-center anchoring on the primary monitor or the monitor under the
  cursor, DPI-aware
- [x] Left or right corner placement; the panel opens toward the other side
- [x] Always on top, or behind all windows (above the desktop)
- [x] Pill 140x34 springs into a 568x236 panel: 4% overshoot, 120 ms hover
  debounce, 250 ms leave grace
- [x] Click, tray or hotkey pins the panel open
- [x] Squircle that morphs from a capsule (pill) to continuous corners
  (panel); hit testing follows it every frame
- [x] Real acrylic blur clipped to the animated shape
  (Windows.UI.Composition), with a solid fallback
- [x] Glass: 70% charcoal gradient, grain, 1 px top highlight, rim. Tint,
  rim and highlight are drawn by the compositor in the same commit as the
  blur, and neither window uses a region, so nothing flashes while
  animating (verified frame by frame with Desktop Duplication)
- [x] Content: fade, 0.96 to 1.0 scale, tiles stagger 20 ms after the shell
  is 60% open; on collapse the content fades out before the shell shrinks
- [x] Tiles: hover lift and glow, press scale 0.97, accent gradient when
  active
- [x] Spring toggles and segmented controls; 120 ms icon crossfade helper
- [x] Idle pill: ink capsule with a local clock and battery (or date on
  desktops), wakes once per minute; swells under the pointer
- [x] Collapsed pill renders module badges (the timer ring replaces the
  clock; glyphs and the pulse dot replace battery or date)
- [x] Repaints only while something moves; idle 0.000% CPU
- [x] Follows the Windows "Animation effects" setting (springs snap)
- [x] Auto-hides behind fullscreen apps
- [x] Tray (Settings, Pin, Launch at startup, Quit); launch at startup via
  the HKCU Run key
- [x] Dark, light and auto themes; wallpaper-derived accent

**Modules**

- [x] Coffee (keep-awake): always or 15 min to 2 h, Ctrl+Alt+C, cup glyph
  in the pill
- [x] Timer: countdown (scroll or +/- to set, sound at zero, snooze) and
  stopwatch, ring and time mirrored in the pill. Pomodoro cycles still to do
- [x] Clipboard history: text and images, in memory only, skips content
  marked private; the expand button opens the full history page
- [x] Color picker: loupe with zoom and pixel nudging, HEX/RGB/HSL/HSV/OKLCH,
  history page, Ctrl+Alt+P
- [x] Media (SMTC): title, artist, album art, progress, previous /
  play-pause / next; a note glyph in the pill while playing

**Settings panel**

- [x] Theme, accent, material, monitor, hover delay, startup
- [x] Module toggles, ordering and sizes (tile editor)

- [x] Text extractor: freeze, drag, Windows OCR, copied with line breaks
  (Shift: one line), Ctrl+Alt+T
- [x] Lens search: freeze, drag, opens Google Lens in the browser,
  Ctrl+Alt+L
- [x] Draw on screen: transparent ink layer over every monitor (pen,
  highlighter, eraser, five colors, undo, clear), draggable toolbar that
  opens on the cursor's monitor, Ctrl+Alt+D to open, Esc to close

- [x] Phone link: LocalSend file and text transfer (discovery, receive
  server, HTTPS client), scrcpy mirroring hosted in the panel with a
  control mode (Esc releases), adb notifications and phone keys, Wi-Fi adb

- [x] QR code: five module styles, four eye styles, gradients, logo and
  label, PNG/SVG/clipboard/drag export, Ctrl+Alt+Q
- [x] Calculator: expression engine with percentages, functions and unit
  conversion, keypad, history, Ctrl+Alt+K
- [x] System monitor: CPU, memory, network charts, drives, top apps
- [x] Calendar: month view, week numbers, events with reminders
- [x] Quick launch: pinned apps, folders, files and sites with shell icons
- [x] Voice memo: microphone to WAV, live waveform, playback, Ctrl+Alt+R

**Quality**

- [x] No `unwrap()` outside tests
- [x] Every Win32 call goes through an error-checked wrapper
- [x] Clippy and fmt clean
- [x] Unit tests for the spring animator, squircle hit-testing and outline,
  bento packer, shell state machine, config, accent extraction and the
  registry
- [x] Unit tests for timer logic
- [x] Unit tests for capture cropping, the snip overlay geometry, OCR
  preprocessing, color formats (OKLCH against reference values) and the
  Lens hand-off page

**Measured** (Win 11 26300, 125% scale, AMD GPU):

- Idle CPU with the clock ticking and the hit-test poller: 0.16% of one
  core over 20 s (0.01% of a 16-thread CPU).
- Idle working set: about 7 MB once collapsed for 3 s (trimmed). Commit
  charge is about 90 MB, mostly the OpenGL driver; see
  [docs/NOTES.md](docs/NOTES.md).

## Contributing and license

See [CONTRIBUTING.md](CONTRIBUTING.md). Nook is released under the
[MIT license](LICENSE).
