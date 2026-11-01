# Progress

## Step 1 — shell style configuration

- Added the `shell_style` config enum and TOML parsing with `pill` as the backward-compatible default.
- Added the spring segmented Style control to Settings; it persists through the existing settings path. Renderer integration is still pending, so the choice does not yet change the shell shape.
- `cargo test --workspace` could not resolve uncached dependencies because access to `index.crates.io` is unavailable. Clippy could not run for the same reason.
- Commit: `6a7711e` (`Add configurable shell style setting`).

## Step 2 — notch geometry

- Added a sampled concave-ear/bottom-rounded polygon outline and an even-odd point-in-polygon helper to `crates/ui/src/geometry.rs`.
- Added geometry bounds and hit-test coverage for the body, ear gap, and outside points.
- The existing shell, compositor, and hit tester still use `Squircle`; carrying this polygon through those APIs remains pending.
- Verification: formatted the edited Rust files. Workspace tests and clippy remain blocked by unavailable crates.io dependencies.
