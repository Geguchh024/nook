# Progress

## Step 26 — Direct tile resize and radius inspector

- Kept the existing tile presets and added a vertical Medium footprint. Plugin tiles receive the new `tall` layout notification, and the plugin docs/checker describe its 1×2 dimensions.
- Added a curved corner grip in layout editing. Dragging snaps to the supported Small, horizontal Medium, vertical Tall and Large footprints; the original drag-to-reorder interaction remains available elsewhere on each tile.
- Clicking a tile in edit mode selects it and expands a compact inspector below the grid. Its continuous slider changes that tile's corner radius from square to a rounded capsule/circle, then shrinks back when editing ends.
- Verification: workspace tests passed (163 passed, 7 ignored); workspace clippy passed with `-D warnings`.

## Step 25 — Select a connected display

- Added a Display mode beside Primary and Follow cursor. Settings lists connected displays with resolution and virtual-desktop coordinates; choosing one updates the live target and is saved.
- The app currently owns one native overlay window, so this step supports targeting one display at a time. Rendering a separate, fully interactive notch on every display needs a multi-window shell architecture and is not represented as a working All screens toggle.
- Verification: workspace tests passed (162 passed, 7 ignored); workspace clippy passed with `-D warnings`.

## Step 24 — OLED theme lock and sound toggle

- Disabled the Theme segmented control while OLED material is selected and added an explanation tooltip; the user's selected theme value remains saved.
- Replaced the sound icon action with the standard on/off toggle in Behavior.
- Verification: workspace tests passed (162 passed, 7 ignored); workspace clippy passed with `-D warnings`.

## Step 23 — Shared button corner style

- Added a Button corner control under Customize. Shared module action buttons and chips now use the adjustable continuous-corner shape, with 13 points as the default.
- The layout editor's separate Size/Radius switch continues to set tile/plugin-container corners individually; the shared control applies to buttons and chips across module pages.
- Verification: `cargo test --workspace --offline --quiet` passed (162 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 22 — Custom background color picker

- Added an arbitrary-color picker for the main glass background alongside the preset swatches.
- Custom section colors now tint elevated surfaces in both dark and light themes; OLED forces an opaque black shell for a true OLED-style panel.
- Verification: `cargo test --workspace --offline --quiet` passed (162 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 21 — Wireless page responsive action and device cards

- Fixed the Wi-Fi and Bluetooth action rows to allocate separate button areas; More settings and Refresh no longer share the same hit rectangle.
- Bluetooth device cards switch to a taller stacked layout on narrow panels, giving device names and status a full line before Pair/Connect/Remove actions.
- Verification: `cargo test --workspace --offline --quiet` passed (162 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 20 — Media badge in the closed shell

- Added a saved Music in closed notch toggle under Customize.
- When enabled, the existing Media module reports a small music glyph while playback is active; the normal clock, connection notices and other badges keep their existing behavior. State comes from the existing SMTC watcher, with no polling loop.
- Verification: `cargo test --workspace --offline --quiet` passed (162 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 19 — Per-tile corner radius editing

- Added a small Size/Radius switch to each tile's layout editor. Radius offers four smooth continuous-corner values (0, 8, 16 and 24 points) without adding another row or overlapping the tile controls.
- Saved radius overrides by module id, so tile/plugin radius survives restarts and follows the tile when the layout is rearranged. The default remains the existing 16-point squircle.
- Verification: `cargo test --workspace --offline --quiet` passed (162 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 18 — Motion controls

- Added a collapsible Motion section beneath Customize with open and close delays, hover glow, animation duration, Smooth/Spring/Bouncy easing and Reset motion.
- The existing hover and collapse delays moved into this section. Duration and easing tune the shell's current spring; animation remains event driven and does not add background polling.
- Verification: `cargo test --workspace --offline --quiet` passed (162 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 17 — Main shell border and shadow

- Added independent Border and Shadow sliders to Customize. Both render around the main shell outline only; they do not add an outline to every tile or button.
- The border reuses the same outline color sent to the acrylic compositor, and the shadow is drawn beneath the shell with the existing squircle shape.
- Reset style restores both shell effects.
- Verification: `cargo test --workspace --offline --quiet` passed (162 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 16 — OLED material, color selectors and pill margin

- Added an OLED material preset that paints the expanded shell pure black with restrained dark surfaces, while keeping Acrylic and Solid available.
- Added custom section and button color pickers, plus a Pill top-margin slider. The margin updates the live shell metrics; Notch mode keeps the control disabled because its shape stays attached to the screen edge.
- Routed custom section colors through the tile/card palette and custom button colors through secondary action buttons.
- Verification: `cargo test --workspace --offline --quiet` passed (162 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 15 — Wi-Fi and Bluetooth responsive page controls

- Reflowed Wi-Fi and Bluetooth status and action controls into wrapping rows so narrow panels do not force the connected state and settings buttons into one line.
- Added truncation to long connected SSIDs and the Wi-Fi connection selection label; reserved more vertical room for the Wi-Fi connect/password controls and allowed its list to shrink on short displays.
- Existing connected Bluetooth devices remain grouped above nearby devices, with their connection and battery state retained in the device cards.
- Verification: `cargo test --workspace --offline --quiet` passed (162 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed after applying Clippy's suggested `clamp` form. Workspace formatter was run; untouched files were then restored.

## Step 14 — Calendar visibility and search refinement

- Restyled the search field and date selector as pill controls with a search icon, selected-date state, and a clear-filter action.
- Sized the inline calendar card to the number of week rows in the selected month instead of forcing a fixed-height grid.
- Notes and Clipboard request additional shell height while the calendar is open; the full Calendar page adds room for busy days. Both layouts still clamp to the active monitor through the shell sizing path.
- Verification: `cargo test --workspace --offline --quiet` passed (162 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed. `git diff --check` passed.

## Step 13 — Responsive calendar and search styling

- Restyled the Notes and Clipboard search field as a rounded, themed pill with a search icon and focus highlight.
- Rebuilt the history date picker as a responsive calendar card with compact day cells, month navigation, and a Today action.
- The full Calendar page now lays out side-by-side when there is enough width and stacks the month and selected-day details on narrower panels.
- Verification: `cargo test --workspace --offline --quiet` passed (162 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed. `git diff --check` passed.

## Step 12 — Notes and Clipboard search/date filters

- Added a shared content search field and expandable month calendar to both Notes and Clipboard pages. Searching is case-insensitive and matches full note or copied-text content; image entries remain filterable by date.
- Picking a calendar day filters entries to that local calendar date; month navigation, Today, and clearing back to All dates are available. Notes keep their current editor selection, and the editor is hidden when that note does not match the active filter.
- The date picker reuses the calendar module's Gregorian day calculations. Clipboard entries record the local date at capture time; Notes derive the local day from the file modification time without adding background polling.
- Verification: `cargo clippy --workspace --all-targets --offline -- -D warnings` and `cargo test --workspace --offline --quiet` passed with workspace-local `APPDATA`.

## Step 1 — shell style configuration

- Added the `shell_style` config enum and TOML parsing with `pill` as the backward-compatible default.
- Added the spring segmented Style control to Settings; it persists through the existing settings path and switches the shell live.
- `cargo test --workspace` could not resolve uncached dependencies because access to `index.crates.io` is unavailable. Clippy could not run for the same reason.
- Commit: `6a7711e` (`Add configurable shell style setting`).

## Step 2 — notch geometry

- Added a sampled concave-ear/bottom-rounded polygon outline and an even-odd point-in-polygon helper to `crates/ui/src/geometry.rs`.
- Added geometry bounds and hit-test coverage for the body, ear gap, and outside points.
- Routed the notch outline through the renderer, Composition clip and hit tester. Notch mode uses monitor bounds at y=0, a 200×32 compact shell, and animates bottom radius into the expanded panel.
- Updated gradient and grain fills to triangulate concave outlines, avoiding fill across the notch ear gaps.
- Raised the Settings appearance card height so the additional Style row has its own space.
- Updated README usage notes for both shell styles.
- Verification: `cargo test --workspace --offline` passed with `APPDATA` redirected to a writable workspace directory. `cargo clippy --workspace --all-targets --offline -- -D warnings` passed. `cargo fmt --all --check` still reports formatting differences in untouched repository files; all edited Rust files were formatted directly.

## Step 3 — Quick Controls

- Added a responsive Controls page and background worker using channels; no idle polling.
- Added Wi-Fi/Bluetooth radio helpers, Windows master volume/mute, and the experimental toast notification registry option.
- Brightness now targets the monitor containing the Nook window through DDC/CI; laptop panels without DDC/CI remain unavailable. Unsupported radios and audio endpoints are omitted from the controls.
- Quick Controls defaults to Small while retaining the existing size editor. Volume now updates optimistically with an unstepped slider and throttled background writes; brightness writes only on release.
- Media opens a dedicated page with album art, current/total duration, a draggable seek bar, and Previous / Play-Pause / Next transport controls.
- Volume is Windows master volume (system-wide); brightness targets the monitor containing the Nook window through DDC/CI. Laptop panels without DDC/CI remain unavailable.
- `cargo test --workspace --offline --quiet` passed (72 modules, 39 platform, 33 UI, 12 app tests; 7 ignored) with `APPDATA` redirected to a writable workspace directory. `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.
- Launched the updated app with the preview config and Settings open. Radio and brightness behavior still needs a hardware session to verify.

## Step 4 — UI sounds

- Added event-driven cues for button clicks, switch on/off, panel expansion/collapse, and media transport. The app bundles the selected user-provided sounds and several original generated WAV cues.
- Added a short slider movement tick for volume, brightness and media seeking, throttled to 65 ms per slider so audio playback remains light while dragging.
- Playback uses the existing Windows `PlaySoundW` path and static embedded WAV data; no background thread or idle polling was added.
- Added `assets/Sounds/generate_ui_sounds.py` to reproduce the original tones. The user-provided MP3s remain alongside the derived, trimmed WAV assets.
- Verification: `cargo test --workspace --offline --quiet` passed (12 app, 72 modules, 39 platform and 33 UI tests; 7 ignored). `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.
- Launched the rebuilt app with the isolated preview config (`shell_style = "notch"`) and Settings open.

## Step 5 — Responsive wireless and Quick Controls

- Removed Wi-Fi and Bluetooth from the Quick Controls page and registered them as independent Small-by-default tiles. Their Small layout is icon-only; wider sizes show the tile name and connection state.
- Added on-demand Wi-Fi scans and Bluetooth discovery/pairing/device removal pages. The Wi-Fi page can connect to open and WPA2-Personal networks; unsupported enterprise/WEP security routes to Windows Settings. Bluetooth connection for already-paired devices routes through Windows Bluetooth Settings because connection methods vary by device profile.
- Added OS event watchers that show the connected/disconnected Wi-Fi network or Bluetooth device name briefly in the notch. These watchers are event-driven; scans only run when a page is opened or refreshed.
- Reworked Quick Controls into a single-column layout with responsive full-width volume and brightness sliders, mute control and the notification toggle. The page remains Small by default and opening it does not resize its tile.
- Added a WMI laptop-panel brightness fallback when DDC/CI is unavailable. This fallback targets the first active WMI brightness endpoint; per-monitor targeting remains available through DDC/CI where supported.
- Updated README controls documentation. Verification: `cargo test --workspace --offline --quiet` passed (12 app, 72 modules, 40 platform and 33 UI tests; 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.
- Hardware behavior for radio discovery, device pairing and laptop brightness still depends on Windows hardware/drivers and needs a device session for end-to-end verification.

## Step 6 — Sound preference and clipboard actions

- Added a saved Sound effects toggle to Settings → Behavior. It defaults on for existing config files and gates every centralized Nook cue, including clicks, panel motion and slider ticks.
- Replaced the clipboard page's fixed four-column grid with one responsive full-width card per history item.
- Hovering a text card exposes Copy, Edit and Delete; image cards expose Copy and Delete. Text editing uses a resizable multiline editor, and Save and copy updates the history item and system clipboard.
- Verification: `cargo test --workspace --offline --quiet` passed with `APPDATA` redirected to a writable workspace directory (13 app, 72 modules, 40 platform and 33 UI tests passed; 7 ignored). `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 7 — Responsive pages and live slider feedback

- Rebuilt Wi-Fi and Bluetooth entries as full-width compact cards with reserved icon and action areas, truncated long names, and scroll regions sized from remaining page height. Wi-Fi reserves room for the selected network's password/connect controls.
- Clipboard cards now show an enlarge action for long text. The edit window grows for long entries, starts focused with the caret at the end, scrolls inside the text field, and has a compact save icon that writes the edited text to the clipboard.
- Volume and brightness retain their optimistic values while asynchronous snapshots arrive, so stale snapshots cannot pull the slider thumb to an old level. Slider writes coalesce queued samples and send periodic latest values while dragging, with a final write on release.
- Added the Windows `CREATE_NO_WINDOW` flag to the WMI fallback so PowerShell no longer flashes during brightness access. Brightness WMI writes are rate-limited during dragging and do not trigger a second WMI query after each write.
- Verification: workspace clippy and tests pass (`cargo clippy --workspace --all-targets --offline -- -D warnings`; `cargo test --workspace --offline --quiet` with workspace-local `APPDATA`: 158 passed, 7 ignored).

## Step 8 — Notch customization, retention and clipboard actions

- Added live Compact, Standard and Large notch sizes plus Classic, Digital and Dot matrix clock rendering.
- Added configurable Notes and Clipboard retention in Settings → Advanced: 1, 3, 5, 7, 10 or 15 days, 1 month, or Never. Notes are recycled when possible and removed otherwise; failed deletion keeps the note and resets its expiry. Clipboard history is in memory, so its retention applies for the current session. Finite retention shows a days-left label.
- Kept clipboard action hit targets active while the pointer moves onto each icon and gives Copy, Edit, and Delete distinct IDs. Clipboard writes now report queue/post failures rather than falsely reporting success.
- Brightness previews use DDC/CI during dragging; laptop-panel WMI applies only the final value on release, avoiding stale slow writes reverting newer positions. Note expiry repaint scheduling ignores unsaved drafts.
- Verification: `cargo test --workspace --offline --quiet` passed (159 passed, 7 ignored) with workspace-local `APPDATA`; `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 9 — Wireless device status and actions

- Wi-Fi network rows now show signal quality as Weak/Fair/Good/Excellent instead of a confusing raw “99%”. Selecting an open or saved network starts connecting immediately; networks needing a new password reveal the password field first.
- Added a More settings action on both wireless pages. Bluetooth details separate connected devices from other paired/nearby devices, and row selection starts pairing or opens Windows' device connection UI for an already paired device.
- Connected Bluetooth devices show a battery percentage when Windows reports the `System.Devices.BatteryLife` property. Battery data is device/driver dependent and absent otherwise.
- Windows does not provide one generic Bluetooth connect operation for every profile, so already paired-device connection is handed to Windows Bluetooth Settings. Wi-Fi connections use WLAN directly where supported.
- Verification: `cargo test --workspace --offline --quiet` passed (159 passed, 7 ignored) with workspace-local `APPDATA`; `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 10 — Sound output and app routing

- Replaced the Sound effects switch with a right-side speaker/mute icon that toggles the saved sound preference live.
- Added an Output and app speakers page under Quick Controls. It lists active render devices, marks the default, enumerates active audio sessions across outputs, and shows each app's display name and shell icon when Windows supplies one.
- Added direct links to Windows Sound settings for default output selection and App volume settings for per-app speaker routing. Nook does not claim to change routes itself: public Core Audio APIs do not expose a supported default-endpoint switch or a generic running-session endpoint reassignment for this unpackaged app.
- Device/session enumeration and icon extraction run only on the controls worker after the page opens or refreshes; the worker waits on its channel at idle.
- Verification: `cargo test --workspace --offline --quiet` passed (159 passed, 7 ignored) with workspace-local `APPDATA`; `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 11 — Settings placement and Bluetooth connected devices

- Moved Hide in fullscreen from Appearance into the right-hand Behavior card.
- Bluetooth now merges paired/nearby results with both classic Bluetooth and Bluetooth LE connected-device selectors, preserving connected state and available battery data when duplicate device IDs are returned.
- The Bluetooth page always labels its Connected devices section and clearly says when none are currently connected.
- Added a regression test ensuring connected-selector data upgrades a paired row instead of being dropped as a duplicate.
- Verification: `cargo test --workspace --offline --quiet` passed (160 passed, 7 ignored) with workspace-local `APPDATA`; `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 12 — Wireless state reads

- Radio state checks no longer require `Radio::RequestAccessAsync` to return permission to change a radio. That permission gate could report Wi-Fi or Bluetooth as off even when the radio was on in Windows.
- Radio changes still request and check access before applying the change; only the read path changed.
- Verification: `cargo test --workspace --offline --quiet` passed (163 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 13 — Media session discovery

- The SMTC worker now searches Windows' complete media-session list and prefers a session with active playback, falling back to the current/first session so paused media remains controllable.
- This covers players that publish a session but are not selected as Windows' single current session; all discovery remains on the existing event-driven media worker.
- Verification: `cargo test --workspace --offline --quiet` passed (163 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 14 — Media tile sizing

- Media now defaults to the Small footprint. Small shows album art (or the media glyph); Medium shows artwork, title and previous/play/next controls; Large retains the detailed player card.
- Wide and Tall Medium footprints reflow controls and artwork to stay inside their bounds, and clicking the artwork or information opens the full media page.
- Verification: `cargo test --workspace --offline --quiet` passed (163 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 15 — Collapsed playback meter

- The collapsed notch now pairs the playing media glyph with three animated audio level bars when the existing “Music in closed pill” option is enabled.
- The renderer schedules 33 ms repaints only while the media badge is visible in the collapsed shell; the idle path remains event-driven.
- Verification: `cargo test --workspace --offline --quiet` passed (163 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 16 — Clipboard copy notice

- Copying a clipboard entry now displays a brief “Copied: …” notice inside the collapsed notch; long text is shortened and copied images show “Copied image”.
- The notice uses the existing timed badge renderer and expires after the same 1.2-second confirmation shown in the Clipboard tile.
- Verification: `cargo test --workspace --offline --quiet` passed (163 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 17 — Collapsed media artwork

- The collapsed notch now uses the current SMTC cover art in place of the clock when Windows supplies it; the media glyph and animated level bars remain visible alongside it.
- Artwork is cached by track and uploaded as an egui texture only when its identity changes; modules reuse shared image bytes between frames.
- Verification: `cargo test --workspace --offline --quiet` passed (163 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 18 — Media footprint classification

- Tightened media tile size detection so a 2×2 Large tile uses the full player; only 2×1 Wide and 1×2 Tall use the compact transport layout.
- Added a focused regression test for Small, Wide, Tall and Large logical footprints.
- Verification: `cargo test --workspace --offline --quiet` passed (164 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 19 — Media access status

- The media worker now reports startup failures to the module channel, and the media page explains when Windows denies system media access instead of claiming playback is idle.
- Verified on this machine that the unpackaged Inno/dev build receives `E_ACCESSDENIED` from SMTC. Microsoft's API documentation requires the `globalMediaControl` package capability; the current installer cannot declare it without moving to Windows package identity.
- README documents this Windows packaging requirement and the current build limitation.
- Verification: `cargo test --workspace --offline --quiet` passed (164 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 20 — Responsive media empty and error page

- Replaced the single painted status line with a centered, width-constrained media page that wraps to the available panel width.
- Windows access failures now have a clear heading, concise explanation, and expandable technical details; the empty state explains how to make media appear.
- Verification: `cargo test --workspace --offline --quiet` passed (164 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 21 — Tile radius reaches circles

- The radius slider now scales to half the selected tile's smaller dimension, so square Small and Large tiles can become circles and rectangular tiles can become capsules.
- The renderer switches to circular corners at the maximum radius, and the inspector hides the overlapping edit hint while open.
- Added radius limit coverage for all four tile footprints.
- Verification: `cargo test --workspace --offline --quiet` and `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 22 — Curved tile resize grip

- Replaced the right-angle resize mark with a high-contrast curved corner arc, widened its drag target, and show the diagonal resize cursor while active.
- Dragging continues to choose the nearest supported Small, Wide, Tall, or Large footprint.
- Verification: `cargo test --workspace --offline --quiet` passed (165 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 23 — Enumerate wireless devices with unknown radio state

- Wi-Fi and Bluetooth scans now proceed when Windows cannot report the radio state, while still skipping scans when the radio is explicitly off.
- Detail pages distinguish an unavailable status from a confirmed Off state and still render device/network results when enumeration succeeds.
- Added a regression test for unknown-radio enumeration; Bluetooth APIs requiring package capabilities can still return no devices in the current unpackaged build.
- Verification: `cargo test --workspace --offline --quiet` passed (166 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 24 — Refine acrylic glass lighting

- Increased the frosted fill and reflective rim on non-OLED acrylic tiles for a clearer layered-glass look inspired by the supplied UI references.
- OLED stays pure black with its original surface, rim, and highlight values; a regression test locks those tokens.
- Verification: `cargo test --workspace --offline --quiet` passed (167 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 25 — Simplify responsive tile editing

- Replaced the dense per-tile tick and miniature size marks with a compact visibility badge, tile name where the footprint has room, and the curved resize grip.
- The selected tile's inspector now groups the four footprint choices and the corner-radius slider; narrow panels use a size dropdown to prevent the segmented control from spilling out.
- Verification: `cargo test --workspace --offline --quiet` passed (167 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 26 — Show connected classic Bluetooth devices

- Added a Win32 connected-device enumeration fallback for unpackaged desktop builds where WinRT Bluetooth selectors may be capability-gated; WinRT continues to provide paired and BLE discovery when available.
- The fallback displays connected names and merges them into matching WinRT records, but does not claim battery data or pairing actions when Windows cannot provide those properties.
- Verification: `cargo test --workspace --offline --quiet` passed (168 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 27 — Scan all Wi-Fi adapters

- User-requested Wi-Fi scans now start on every enumerated WLAN adapter and combine duplicate SSIDs using the strongest signal and connected state.
- Connecting to a selected SSID now uses the adapter that reported it instead of always choosing the first adapter.
- Added coverage for combining per-adapter network results.
- Verification: `cargo test --workspace --offline --quiet` passed (169 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 28 — Reduce background watcher sampling

- Kept fullscreen, coverage, and monitor checks on the existing one-second watcher cadence, while moving theme, animation, transparency, battery, and wallpaper reads to a ten-second cadence.
- The watcher still wakes the UI only when sampled state changes; there is no UI-thread polling or additional worker loop.
- Verification: `cargo test --workspace --offline --quiet` passed (169 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 29 — Restyle Wi-Fi and Bluetooth tiles

- Redesigned only the tile faces: compact tiles use an icon badge, wide tiles show the radio name and connection summary side by side, and larger tiles add a brief device/network count.
- Active radios now use a subtle accent badge and rim while keeping the existing glass tile surface. The existing tile click, refresh, device discovery, and detail pages are unchanged.
- Verification: `cargo test --workspace --offline --quiet` passed (169 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 30 — Add shape-aware tile layout context

- Added shared size class, orientation, and rounded/circle shape metadata to `TileCtx`, plus conservative safe content bounds.
- The bento renderer now constrains tile content to those bounds, including an inscribed content region for circle-shaped square tiles. Added geometry/classification tests.
- Existing module interaction and radio behavior remain unchanged; individual modules can use the new context for their responsive variants.
- Verification: `cargo test --workspace --offline --quiet` passed (172 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 31 — Adapt collapsed media to notch size

- Media badges now carry cached title/artist text and live timeline progress without allocating fresh strings every frame.
- Compact notch shows art, active playback bars, and a thin progress line; Standard adds the title; Large also reserves a clock region and shows artist text.
- Playback animation/repaint is capped at 4 Hz while media is active; the existing SMTC worker remains event-driven.
- Verification: `cargo test --workspace --offline --quiet` passed (172 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 32 — Open the active media source

- The app/source label in the media tile and full media page is now clickable and opens the owning app through its SMTC app id.
- Packaged app ids use the Windows AppsFolder namespace; classic `.exe` source ids are validated before launch. Failures are logged without interrupting playback controls.
- Added tests for accepted and rejected launch targets. The SMTC worker and its event-driven discovery remain unchanged.
- Verification: `cargo test --workspace --offline --quiet` passed (173 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 33 — Add per-corner tile radius editing

- Added independent TL/TR/BR/BL radii with CSS-style edge clamping, per-tile config persistence, and automatic migration from legacy uniform radius values.
- The edit inspector now has linked/unlinked corner sliders and a live outline preview. The same radii drive tile drawing and tile interaction checks; the radius slider remains available and uses accurate Circle/Pill labels by footprint.
- Added geometry, serialization, and legacy migration tests.
- Verification: `cargo test --workspace --offline --quiet` passed (177 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 34 — Fix responsive Settings rows

- Settings rows now reserve a fixed, measured label column and give controls their own right-aligned area, preventing the Monitor label from overlapping its choices.
- Narrow Monitor layouts switch to a compact dropdown, and the display picker is sized to its available column so its popup and selection remain usable on constrained panels.
- Verification: `cargo test --workspace --offline --quiet` passed (177 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 35 — Add experimental Airplane tile

- Added a separate Airplane tile that turns off available Wi-Fi/Bluetooth radios and restores the recorded prior state when disabled. Radio work runs on its own initialized worker; state is sampled only when its detail page opens or after an action.
- The saved restore state is persisted in the Nook config. If no prior state exists, turning Airplane off restores both available radios.
- The detail page clearly states that this is a radio off/restore helper; Windows' system airplane indicator may not change. Existing Wi-Fi and Bluetooth modules were not modified in this phase.
- Verification: `cargo test --workspace --offline --quiet` passed (178 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 36 — Add microphone mute control

- Quick Controls now reads and toggles the default communications capture endpoint's mute state through `IAudioEndpointVolume` on the existing controls worker.
- The control is hidden as unavailable when Windows has no usable microphone endpoint; no microphone state is fabricated.
- Verification: `cargo test --workspace --offline --quiet` passed (178 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 37 — Add Windows theme Quick Control

- Added a Windows dark-mode toggle that reads and updates both `AppsUseLightTheme` and `SystemUsesLightTheme` for the current user.
- Nook broadcasts `WM_SETTINGCHANGE` with a bounded timeout after writing the preferences; broadcast failures are logged and do not stall the controls worker.
- The toggle is unavailable when Windows has no readable preference value.
- Verification: `cargo test --workspace --offline --quiet` passed (178 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 38 — Make Settings cards responsive

- Appearance and Behavior now stack at compact panel widths, leaving the segmented settings and monitor controls their full card width. Wider panels retain the two-column layout.
- The settings page height now accounts for the stacked cards, including the additional display selector row, instead of laying controls beyond a fixed-height card.
- Verification: `cargo test --workspace --offline --quiet` passed; `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 39 — Choose one or more target displays

- Added a monitor picker with a checkbox for each connected display. One selected display pins the notch there; with multiple selected, the single notch follows the cursor only among the selected screens. The picker states this behavior explicitly.
- Existing `primary`, `cursor`, and single `display` config values remain supported. The new `selected_displays` mode persists a sorted list of display indices and safely falls back if a selected screen disconnects.
- Verification: `cargo test --workspace --offline --quiet` passed (180 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 40 — Add a Lock PC Quick Control

- Added a Lock now action that calls the checked Windows `LockWorkStation` API from the existing Quick Controls worker thread. Failures flow through the normal worker error log.
- Verification: `cargo test --workspace --offline --quiet` passed (180 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 41 — Show battery status in Quick Controls

- The on-demand Quick Controls snapshot now includes battery percentage and charging state. A compact status row appears only when Windows reports a battery; desktops without one show no placeholder or fabricated value.
- Verification: `cargo test --workspace --offline --quiet` passed (180 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 42 — Add Windows Mobile hotspot control

- Quick Controls now checks the active Internet connection profile and exposes Mobile hotspot only when Windows reports tethering capability. Start/stop runs through WinRT on the existing worker, and operation failures are returned to its normal error path.
- Verification: `cargo test --workspace --offline --quiet` passed (180 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 43 — Make circular media tiles shape-aware

- Circle-shaped media tiles now clip cover art to a true circle, draw a playback progress ring, and provide a centered play/pause action; tapping elsewhere opens the full media page. The idle state uses the same restrained glass treatment.
- Verification: `cargo test --workspace --offline --quiet` passed (180 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 44 — Keep Quick Controls usable in short panels

- The main controls page now scrolls inside its available panel body height, so optional rows such as battery, microphone, theme, lock, and hotspot remain reachable on shorter displays instead of extending past the shell.
- Verification: `cargo test --workspace --offline --quiet` passed (180 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 45 — Adapt radio tiles to Circle and footprint

- The shared Wi-Fi, Bluetooth, and Airplane tile face now uses the renderer's `TileShape` and `SizeClass`. Small Circle tiles stay icon-only; larger Circle tiles center the title and connection summary, with extra detail reserved for Large. Rounded tiles keep their existing horizontal pill and stacked layouts.
- Tile activation and full-page navigation are unchanged.
- Verification: `cargo test --workspace --offline --quiet` passed (180 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.

## Step 46 — Respect Small radio tile sizing

- The shared wireless face now keys its icon-only state to the actual Small footprint rather than a fixed pixel threshold, so DPI scaling does not unexpectedly add labels to compact tiles.
- Verification: `cargo test --workspace --offline --quiet` passed (180 passed, 7 ignored); `cargo clippy --workspace --all-targets --offline -- -D warnings` passed.
