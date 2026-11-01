# Progress

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
