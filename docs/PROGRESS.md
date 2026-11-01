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
- Controls module (step 3) remains to be implemented.

## Step 3 — Quick Controls

- Added a responsive Controls page and background worker using channels; no idle polling.
- Added Wi-Fi/Bluetooth radio helpers, Windows master volume/mute, and the experimental toast notification registry option.
- Brightness is reported unavailable because no WMI or DDC/CI implementation is present. Unsupported radios and audio endpoints are omitted from the controls.
- Workspace tests and workspace Clippy pass with the controls module and added Windows features.
- Brightness remains unavailable: this build has no WMI or DDC/CI implementation. Radio and volume behavior still needs a hardware session to verify.
