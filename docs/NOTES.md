# Engineering notes

Limits hit while building the shell on egui/eframe, how each one is handled,
and the defaults chosen where the spec left room. Measured on Windows 11
build 26300, 1920x1080 at 125% scale, AMD GPU.

## egui / Windows limitations and workarounds

### 1. Per-pixel transparency
- **Problem:** eframe's wgpu backend on Windows presents through a DX12/Vulkan
  swapchain that DWM composites as opaque, so a transparent window comes out
  black.
- **Workaround:** the `glow` (OpenGL) backend, `with_transparent(true)` and
  `App::clear_color` returning `[0,0,0,0]`. winit then enables DWM
  blur-behind with an empty region, which turns on per-pixel alpha.

### 2. Click-through outside the shape
- **Problem:** egui and winit only offer all-or-nothing mouse passthrough.
  `WM_NCHITTEST -> HTTRANSPARENT` only forwards to windows on the same
  thread.
- **First approach, dropped: a window region** (`SetWindowRgn` with the
  squircle outline). It is the cheapest hit-test there is, but every region
  change on the OpenGL window made DWM paint the newly included area a pale
  blue-white (about `#D5E3F1`) for one to three frames before GL caught up.
  That was the white flash at the corners while the shell opened and
  closed. Desktop Duplication captures (every composed frame at 144 Hz)
  showed it on 67 of 250 frames. Re-applying DWM blur-behind after each
  change did not help; holding the region fixed made it disappear.
- **Chosen:** the window has **no region**. It is layered
  (`WS_EX_LAYERED`, alpha 255, so per-pixel GL alpha still works), and
  `WS_EX_TRANSPARENT` is flipped when the cursor crosses the shape:
  - `platform::hittest` polls `GetCursorPos` on a small thread: every 8 ms
    near the shape, every 40 ms elsewhere. It only *decides*, then wakes
    the UI.
  - The UI thread applies the style. Changing a window's style from
    another thread sends a synchronous message to the window's thread; the
    first version did that while holding a lock the UI thread also wanted,
    and deadlocked on the first frame.
  - Each flip is followed by `SetWindowPos(SWP_FRAMECHANGED)`. Without it,
    Windows kept hit-testing with the old style and presses never arrived
    after the first toggle.
  - A button held over the shape keeps the window interactive until it is
    released.
- **Synthesized events:** once the window is click-through no more mouse
  messages arrive, so the app pushes `PointerGone` to egui (deferred by a
  frame when that frame carries a click). When it becomes interactive it
  pushes the current cursor position, so a resting cursor hovers without
  having to move.
- **Cost:** 0.16% of one core at idle, including the clock tick (0.01% of
  the machine).

### 3. Backdrop blur (the hard one)
- **Problem:** egui cannot sample or blur what is behind the window.
- **Tried first: `SetWindowCompositionAttribute`
  (`ACCENT_ENABLE_ACRYLICBLURBEHIND`).** On this Windows 11 build it fills
  the *whole window rectangle* and ignores the window region. Re-applying it
  after the region changes does not help. **Dropped.**
- **`DWMWA_SYSTEMBACKDROP_TYPE`** (Mica/Acrylic) is likewise rectangular,
  with only DWM's fixed 4/8 px corners. Not usable for a morphing squircle.
- **Chosen: Windows.UI.Composition** (`platform::backdrop`):
  - A `SpriteVisual` painted with `Compositor::CreateHostBackdropBrush`
    (the blurred desktop, the same source acrylic uses).
  - The visual is clipped by a `CompositionGeometricClip` built from a
    Direct2D path geometry of the same squircle outline. Clipping is
    sub-pixel and anti-aliased, and follows the spring every frame.
- **Second gotcha:** a `DesktopWindowTarget` attached to the OpenGL window
  draws *above* the GL content whatever `isTopmost` says. Verified with a
  red test brush, under which the tiles showed. The blur therefore lives in
  a second window (`WS_EX_NOREDIRECTIONBITMAP`, tool, no-activate) kept
  exactly under the main window. The main window is made an **owned**
  window of it (`GWLP_HWNDPARENT`), which pins the z-order. The backdrop
  window has no region either (it showed the same pale flash): it is
  layered and permanently `WS_EX_TRANSPARENT`, and only the composition
  clip gives it a shape.
- **Third gotcha: the edge flash.** OpenGL (main window) and Composition
  (backdrop window) present on separate paths that cannot be synchronized.
  While the tint, rim and highlight were drawn in GL, they ran one or two
  frames ahead of the composition clip. Over bright backgrounds a pale ring
  flashed around the shape on every open and close (caught with 10 ms
  screen-capture sequences).
  - **Fix:** everything that defines the *edge* is in the compositor, under
    one clip, in one commit: the blur sprite, the tint sprite (vertical
    gradient), and a `ShapeVisual` with the rim stroke (same path geometry)
    and the top highlight (a line with a fading gradient stroke).
  - GL draws only the film grain (about 3% alpha) and content that sits
    well inside the edge (tiles, pill text), so a frame of lag there is
    invisible.
  - That fixed a pale *ring*. The remaining white *corners* came from window
    regions; see section 2.
- **Fallback:** if Composition fails, the GL path paints the tint, rim and
  highlight itself (`paint::glass` with `fill = true`). If "Transparency
  effects" is off in Windows settings, or the user picks Solid, the
  compositor keeps the tint and hides only the blur layer (96% opacity).
- **Blur strength animation:** the host backdrop blur radius is fixed by
  the system. Instead of animating blur strength, the glass tint opacity
  and the shell opacity animate. A custom blur needs a
  `CompositionEffectBrush` (Win2D `GaussianBlurEffect`) over a
  `CreateBackdropBrush`. That is possible but needs a Win2D effect-graph
  shim; deferred.

### 4. Soft outer shadow
- **Problem:** egui only paints the shell's own glass; a shadow around it
  would sit over other apps and needs its own click-through handling.
- **Current state:** a 1 px rim plus the top highlight (both in the
  compositor) stand in for it.
- **Planned:** a Composition `DropShadow` on the backdrop window's visual.
  That window is already layered and click-through, so the shadow can
  extend past the shape without catching clicks.

### 5. winit rewrites window styles
- **Problem:** after eframe makes the window visible, winit re-applies its
  own styles. Measured: the extended style went from
  tool/no-activate/topmost to `WS_EX_APPWINDOW`. That briefly allowed focus
  stealing and a taskbar entry.
- **Workaround:** `platform::window::ensure_overlay` runs every frame. It is
  one `GetWindowLongPtrW` and only writes when the bits drifted. It also
  clears `WS_EX_NOACTIVATE` while egui wants keyboard input (future text
  fields).

### 6. Hidden windows stop `ui()`
- **Problem:** eframe skips the UI pass while the window is hidden.
- **Workaround:** fullscreen auto-hide fades the shell out, then hides the
  compositor glass and empties the hit shape instead of hiding the window.
  The event loop keeps running, and coming back is a normal spring.

### 7. Scaling content
- **Problem:** egui has no per-widget transform.
- **Workaround:** each bento tile is its own `Area` (layer), and
  `Context::set_transform_layer` applies a `TSTransform` per tile: the
  content scale 0.96 to 1.0 out of the anchored top edge, the stagger slide, the
  hover lift and the press scale 0.97. Input is transformed too, so hit
  areas match what you see.

### 8. Fixed-size, full-width OS window
- **Problem:** resizing a transparent GL window every frame causes
  swapchain reallocations and visible jitter. Moving it means the GL window
  and the backdrop window move in separate steps, so for a frame the glass
  and its content would not line up.
- **Workaround:** the OS window spans the full width of the monitor's work
  area and is as tall as the expanded panel plus 8% overshoot. Only the
  drawn shape and the compositor clip animate. The placement (left corner,
  center, right corner) is a spring on the shape's horizontal anchor inside
  that window, so switching it slides the notch across; nothing moves at
  the OS level. A notch in a corner grows away from its edge, and the panel
  content scales out of that top corner.

### 9. Fonts and tabular numerals
- **Problem:** egui cannot enable OpenType features (`tnum`). Inter's
  default figures are proportional, so timer digits would jitter.
- **Choice:** Nook loads the system's **Segoe UI** (regular and semibold),
  whose digits are tabular by default, from `%WINDIR%\Fonts` at runtime.
  Nothing is bundled and there are no licensing concerns.
- **Fallback:** egui's built-in font if those files are missing.
- **Icons:** Phosphor regular and fill via `egui-phosphor` 0.14, which pins
  egui and eframe to **0.36**.

### 10. Memory
- **Problem:** the idle RAM target is under 60 MB. Measured with the panel
  collapsed: about 100 MB working set, of which the AMD OpenGL driver
  (`atio6axx.dll`) alone maps 62 MB. Any OpenGL app pays this.
- **Workaround:** after the shell has sat collapsed and untouched for
  3 s, Nook calls `K32EmptyWorkingSet` (`platform::process`). It repeats
  after each idle repaint (the pill clock ticks once a minute), never while
  the pointer is on the notch. Measured idle working set: **about 6 MB**,
  at **0.08% CPU over 20 s** (the window included a minute tick).
- **Remaining cost:** commit charge stays around 90 MB (driver
  allocations). Pages fault back in from RAM on the next expand.
- **To do:** compare with an Intel/NVIDIA driver.

### 11. Repaint discipline
- Nothing calls `request_repaint` unless a spring is unsettled, the stagger
  reveal is running, a timer deadline (hover debounce, collapse grace,
  trim) is pending, or a module asks through `Module::next_repaint`.
- The idle pill shows a clock, so it wakes exactly once when the minute
  changes (`system::clock().secs_to_next_minute`).
- The system watcher (fullscreen, monitor, theme, animation and
  transparency settings) polls on its own thread at 1 Hz and wakes the UI
  only when something changed.
- The `debug-overlay` feature shows the frame count, CPU time per frame,
  the frame interval and the first repaint cause.

### 12. Z-order: on top, or behind everything
- **Behind:** `HWND_NOTOPMOST`, then `HWND_BOTTOM` for the main window and
  then its owner (the backdrop window). That puts Nook under every app
  window but still above the desktop (`Progman`), so it shows on an empty
  desktop.
- **Back on top, the gotcha:** `SetWindowPos(HWND_TOPMOST)` returned success
  and did nothing. Windows refuses to make a window topmost while it *owns*
  a window that is not topmost. Here that is the hidden IME window Windows
  attaches to winit's window, and also the main window itself, which the
  backdrop window owns. Fix: detach the owned windows, promote the owner
  and the main window, then restore ownership. A topmost owner keeps its
  non-topmost owned windows once they are re-attached.
- The topmost bit is checked every frame and re-applied if something
  changed it.

### 13. Screen tools: freeze, select, recognize
Text extractor, lens search and the color picker share one overlay
(`platform::snip`), modelled on PowerToys Text Extractor and ShareX.
- **Freeze first.** The whole virtual screen (every monitor) is grabbed
  with `BitBlt` (`platform::capture`), then shown in one topmost popup
  spanning it. Menus, tooltips and video hold still while you select, the
  pixels you pick are exactly the ones you saw, and a selection can cross
  monitors.
- **Mixed DPI.** The process is per-monitor aware, so the popup and the
  grab are in physical pixels across monitors with different scaling.
  Labels are not: the hint, size label and loupe follow the monitor under
  the cursor, each with a font and sizes for that monitor's DPI
  (`GetDpiForMonitor`), and the loupe flips at that monitor's edges so it
  never straddles a seam. Gaps in an uneven layout are covered by the popup
  but invisible.
- **Memory.** Only the frozen frame and (for a region) its dimmed copy are
  kept at full size; the back buffer is allocated per `WM_PAINT` at the
  size of the dirty rectangle. Two 4K monitors cost about 130 MB while the
  overlay is open, freed when it closes.
- **Nook stays out of the grab.** Its own windows get
  `WDA_EXCLUDEFROMCAPTURE` for the moment of the copy (two `DwmFlush`es so
  DWM composes without them), then the old affinity comes back. The open
  panel never shows up in a selection, and Nook stays visible to other
  screen recorders the rest of the time.
- **Plain GDI, not egui.** The overlay is opaque (it shows the frozen
  frame), so it does not need the transparent-window tricks of sections 1
  to 3. The frame and a dimmed copy live in memory DCs. `WM_PAINT` only
  recomposes the invalidated rectangles (old and new selection, label,
  loupe) through a back buffer, so a 4K drag costs a few small blits and
  never flickers. The loupe is `StretchBlt` with `COLORONCOLOR`, which
  gives hard-edged magnified pixels for free.
- **`DrawTextW` and empty slices.** windows-rs passes an empty `&mut [u16]`
  as a dangling pointer with length 0, and user32 still reads it: an
  access violation inside `user32.dll` that killed the process the first
  time the picker drew its blank label pill. Empty text is skipped.
- **OCR: `Windows.Media.Ocr`.** Microsoft's docs say the namespace needs
  package identity. It does not: a plain unpackaged exe reads a 1200x500
  grab in about 60 ms (the identity requirement applies to the newer
  Windows AI `TextRecognizer`). It reads the languages whose OCR packs are
  installed. Small selections are upscaled (bilinear, up to 3x) and padded
  with their border color, since the engine misses small text and glyphs
  touching the edge. Chinese and Japanese results come back with a space
  between characters, which is removed. Tesseract, PaddleOCR (RapidOCR)
  and `ocrs` were considered; all need bundled models (tens of MB) for no
  clear gain on screen text.
- **Lens: the browser does the upload.** Nook still makes no network calls.
  The selection goes into a temp page that posts it to
  `lens.google.com/v3/upload` (`encoded_image`, as Chrome's context menu
  and the "Search on Google Lens" add-on do) and opens in the default
  browser. Lens answers **403** to an upload that carries a `Referer`, so
  the page sets `no-referrer`. The page is deleted after a minute, and
  leftovers are swept on the next search. `AllowSetForegroundWindow` hands
  Nook's foreground right to the browser so it comes to the front. The
  form is undocumented: if it breaks, `modules::lens::page` is the one
  place to change.
- **Threads.** Each overlay runs its own message loop on its own thread.
  The tool's work (OCR, encoding) runs there after the window is gone and
  reports back through a channel the module drains in `on_tick`
  (`modules::snipper`), so the UI never waits.

### 14. Draw on screen across monitors
- The ink canvas spans the virtual screen, so you can draw on any monitor
  and strokes can cross from one to the next.
- **Dirty-rect presents.** `UpdateLayeredWindow` copies the whole bitmap
  every call: about 16 MB per pen move for two 1080p monitors. Presents now
  go through `UpdateLayeredWindowIndirect` with `prcDirty` set to the
  rectangle the last segment touched, so a pen move costs what it draws.
  Undo, erase and clear still present the whole canvas.
- **Toolbar placement.** egui places a new viewport with the main window's
  scale, which is wrong on a monitor with another one. Once the toolbar
  window exists it is moved with `SetWindowPos` in physical pixels to the
  bottom center of the monitor under the cursor, sized for that monitor's
  DPI; Windows then sends `WM_DPICHANGED` and winit rescales its content.
- **Testing tip.** A test driver that is only system-DPI-aware gets its
  `SetCursorPos` coordinates rescaled on monitors whose scaling differs
  from the primary's, which looks like an offset bug in Nook. Drive input
  from a per-monitor-aware process
  (`SetProcessDpiAwarenessContext(-4)`).

### 15. Phone link: a guest window on the panel
- **Mirroring is scrcpy's own window**, not decoded video in egui (that
  would mean an H.264 decoder and scrcpy's control protocol in Nook). It is
  started borderless at the phone area, found by its unique title and pid,
  then adopted (`platform::embed`): popup and tool-window styles, a rounded
  window region, and **owned by the main window**. Owned windows stay
  directly above their owner whatever the z-order does, so the mirror
  follows Nook to the top or behind other apps. `window::is_covered`
  ignores windows owned by the panel, or the watcher would see the mirror
  as an app covering Nook and keep re-raising Nook over it.
- **View only = click-through.** The guest is layered at alpha 255 with
  `WS_EX_TRANSPARENT`, so clicks fall through to the page, where a click on
  the mirror starts control. Control drops the bit and hands the guest the
  foreground (briefly attaching to the foreground thread's input, since
  Nook never activates). A `WH_KEYBOARD_LL` hook, installed only while
  controlling, swallows Esc (down and up) when the guest has the
  foreground and ends control.
- **Never hide the guest.** After `ShowWindow(SW_HIDE)` and a show, scrcpy's
  OpenGL surface stays black (measured: average brightness 0.6 against
  39.6 before; style changes, ownership, resizing and the region were each
  ruled out the same way). Hiding uses the layered alpha (0 or 255), which
  scrcpy never notices. The mirror is shown only while the page is fully
  opaque and its clip covers the phone area, and stops after 20 s hidden.
- **Taller pages.** `Module::page_size` lets a page grow the panel
  (800 x 560 for phone link). The OS window grows at once and shrinks only
  after the shape has come back down: two resizes per visit, not one per
  frame. Progress stays measured against the regular panel, so the glass
  does not re-run its pill-to-panel blend, and page content is clipped to
  the current shape, so it unrolls with the glass.
- **DPI.** scrcpy runs with `SDL_WINDOWS_DPI_AWARENESS=permonitorv2`, so
  its window is in physical pixels like Nook's and is not bitmap-stretched.
- **adb without `adb.exe` per command.** The adb server's socket protocol
  (`127.0.0.1:5037`): `host:track-devices-l` is one long-lived connection
  the server writes to on every change, and shell commands go through
  `host:transport:<serial>` + `shell:`. Notifications come from `dumpsys
  notification --noredact` every 8 s (3 s while the page is open), and the
  UI only wakes when the list changed. The adb shipped with scrcpy is
  preferred: scrcpy restarts a server of another version.
- **LocalSend.** Nook advertises `"protocol": "http"`, so peers talk plain
  HTTP to its `tiny_http` server. Phones serve HTTPS with self-signed
  certificates, so the `ureq` client accepts any certificate (handshake
  signatures are still checked). Uploads stream with a `Content-Length`
  and count bytes as they go; received files are written as `.nookpart`
  and renamed when complete, and peer file names are sanitized (no `..`,
  drives or reserved characters). Tested end to end in-process and against
  an HTTPS stand-in (`cargo test -p nook-modules -- --ignored`).

## Defaults chosen (spec left room)

| Topic | Choice | Why |
|---|---|---|
| Toolchain | `stable-x86_64-pc-windows-msvc` via `rust-toolchain.toml` | Requested. Needs VS 2022 Build Tools (C++ workload, Windows 11 SDK). |
| egui version | 0.36 | Newest that `egui-phosphor` supports. |
| Idle pill | Ink capsule in every theme, blending into the theme glass as it opens. Left: local time in the Windows locale format. Right: battery icon and % (green bolt while charging), or the short date on machines without a battery. | Gives the pill a job and a hardware-notch identity. Module badges replace the clock (timer ring) and the right side (glyphs, pulse). |
| Hover intent | The closed pill swells by 12 x 2 pt while the pointer rests on it (during the 120 ms debounce) | Immediate feedback before the panel opens. |
| Pill position | Floating 6 pt below the top of the monitor's work area; 6 pt from the side in a corner | Reads as a notch without touching the edge, and stays clear of a top taskbar. |
| Placement | Center by default; Left or Right corner in Settings (`placement`). Switching slides it across with a 3% overshoot spring | Corners suit wide monitors and keep the center free for browser tabs. |
| Always on top | On by default (`on_top`). Off puts it behind app windows, above the desktop. Ctrl+Alt+N always brings it back on top | A notch that sits on the desktop like a widget is useful too; the hotkey is the way back when it is covered. |
| Panel size | 568 x 236 | 8 x 3 grid of 60 px cells, 8 px gaps and 16 px padding. The 24 px bottom gutter holds the hover caption ("Media · Coming soon"). |
| Expanded radius | 32 with a corner exponent of 4 | A squircle corner looks smaller than a circular one of the same radius. The pill uses exponent 2 (true capsule) and morphs to 4 as it opens. |
| Shell spring | Width k=260, height k=220, both 4% overshoot | Height trails width slightly, so the morph feels organic rather than linear. |
| Pin | Click on the glass (not a tile), the tray, or Ctrl+Alt+N | A pushpin glyph at the top center shows the pinned state. |
| Toggle hotkey | `ctrl+alt+N` | Unused by Windows and common apps. Configurable as `toggle_hotkey`. |
| Accent | Wallpaper dominant hue (saturation-weighted histogram), else `#7C5CFF` | Windows accent and fixed violet are options in Settings. |
| Hover delay presets | Fast 60 / Normal 120 / Calm 250 ms | Normal matches the spec. |
| Reduced motion | Springs snap and tweens drop to 0 ms | Follows "Animation effects" (`SPI_GETCLIENTAREAANIMATION`). |
| Single instance | Named mutex `Local\Nook.SingleInstance` | A second launch exits quietly. |
| Logs | `%APPDATA%\Nook\nook.log` (release), stderr (debug) | Level via `NOOK_LOG`, e.g. `NOOK_LOG=debug`. |
| Config errors | A bad file is renamed `config.toml.bad` and defaults are used | Never crash on a typo. |
| Dev flags | `--pinned`, `--settings`, `--page=<module id>` | Start expanded (or on the settings or a module's page) for screenshots. |
| Screen tool hotkeys | Text extractor `ctrl+alt+T`, color picker `ctrl+alt+P`, lens `ctrl+alt+L` | Same Ctrl+Alt family as the others, and all were free here. PowerToys uses Win+Shift+T and Win+Shift+C, so both can run side by side. |
| Color formats | HEX by default; RGB, HSL, HSV, OKLCH in modern CSS syntax (`rgb(18 154 227)`) | Pastes straight into CSS; OKLCH is what design tools now use. |
| Switch | 36 x 22, knob inset 3 px with a shadow, off track with a rim | The old 34 x 20 knob nearly filled the track, and the off state read as a floating dot on dark glass. |
