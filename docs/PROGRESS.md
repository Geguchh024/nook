# Progress

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
