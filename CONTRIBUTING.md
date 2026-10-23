# Contributing to Nook

Thanks for helping. There are two ways to add something to the notch:

1. **A plugin** (most ideas): a folder with `plugin.toml` and a program in
   any language. You don't need to change Nook or know Rust. Read
   [docs/plugins.html](docs/plugins.html) and start from one of
   [examples/plugins](examples/plugins). Publish it in your own repository
   with the `nook-plugin` topic. Send a pull request here only if it is a
   small, dependency-free example that teaches something new.
2. **A built-in module or a change to Nook itself**: Rust, in this repository.

## Building

Windows 10/11, Rust stable with the MSVC toolchain (pinned in
`rust-toolchain.toml`) and the Visual Studio 2022 Build Tools with the C++
workload. See the README.

```bash
cargo run --release
```

Before opening a pull request:

```bash
cargo fmt --all
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo test --workspace
```

## House rules

These keep Nook at 0% CPU when idle and stop it from ever freezing the desktop:

- Nothing on the UI thread may block. Do the work on a thread and drain a
  channel in `Module::on_tick`.
- Repaint only when something changes: `Module::next_repaint` returns `None`
  when idle.
- No `unwrap()` outside tests. Every Win32 call goes through an
  error-checked wrapper in `crates/platform`.
- No network calls from Nook itself. Plugins may make their own and must say
  so in their README.
- When you hit a Windows or egui limit, write it up in
  [docs/NOTES.md](docs/NOTES.md) with the workaround.
- Match the surrounding code: short doc comments that explain *why*, and
  plain names.

## Changing the plugin protocol

The host is in `crates/modules/src/plugins/`:

| File | What it holds |
|---|---|
| `manifest.rs` | `plugin.toml` fields and plugin discovery |
| `protocol.rs` | Every message and view component (serde types) |
| `view.rs` | How components are drawn and which events they send |
| `host.rs` | Starting programs, pipes, poll mode, dev-mode reload |
| `mod.rs` | The `Module` implementation that ties it together |

Changes must stay backward compatible: add optional fields and new
component or message types. Never rename or remove existing ones. Older
Nooks draw unknown components as nothing, and plugins must ignore unknown
messages. Update `docs/plugins.html`, both SDKs in `sdk/`, and add a test in
`protocol.rs` for any new type. The Claude Code skill in `skills/nook-plugin`
describes the protocol too: update `references/` and the field tables in
`scripts/check_plugin.py`, then run `python skills/pack.py` to refresh its
SDK copies, icon list and the zip the website serves.

## Reporting bugs

Include your Windows version, display scaling, the relevant lines of
`%APPDATA%\Nook\nook.log` (run with `NOOK_LOG=debug` for more), and for
plugin bugs the plugin's `plugin.toml`.
