//! Lens search: drag over anything on screen and search for it with Google
//! Lens in your browser, like Circle to Search.
//!
//! Nook itself still makes no network calls. The selection is written to a
//! small page in the temp folder that posts the image to Lens's upload form
//! (the way the Search on Google Lens browser add-on does) and opened in
//! the default browser, so the upload happens there, only when you ask,
//! and you see the results as a normal Lens page. The page is deleted a
//! minute later. Ctrl+Alt+L starts it from anywhere.
//!
//! The upload form is not a documented API; if Google changes it, this is
//! the one place to update.

use nook_platform::capture::Frame;
use nook_platform::shell;
use nook_platform::snip::{SnipMode, SnipOutcome};
use nook_ui::icons;
use nook_ui::module::{Module, TileCtx, TileSize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::snipper::Snipper;

const HINT: &str = "Drag over what you want to search  \u{00B7}  Esc to cancel";
const UPLOAD_URL: &str = "https://lens.google.com/v3/upload";
/// Longest edge sent; Lens scales larger images down anyway.
const MAX_EDGE: u32 = 1600;
const QUALITY: u8 = 90;
/// How long the hand-off page stays in the temp folder.
const KEEP_PAGE: Duration = Duration::from_secs(60);
const PAGE_PREFIX: &str = "nook-lens-";

#[derive(Default)]
pub struct Lens {
    snipper: Snipper<bool>,
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ABC[(n >> (18 - 6 * i)) as usize & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The hand-off page: posts `jpeg` to the Lens upload form on load. Lens
/// answers 403 to an upload that carries a `Referer`, so none is sent.
fn page(jpeg: &[u8], stamp: u128) -> String {
    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="referrer" content="no-referrer"><title>Google Lens</title>
<style>html{{color-scheme:light dark}}body{{margin:0;height:100vh;display:grid;place-items:center;font:15px "Segoe UI",system-ui,sans-serif;opacity:.7}}</style>
</head><body><p>Opening Google Lens&hellip;</p>
<form id="f" method="POST" enctype="multipart/form-data" action="{UPLOAD_URL}?ep=ccm&amp;s=&amp;st={stamp}">
<input type="file" name="encoded_image" id="i" hidden></form>
<script>
const b = atob("{data}");
const u = new Uint8Array(b.length);
for (let k = 0; k < b.length; k++) u[k] = b.charCodeAt(k);
const t = new DataTransfer();
t.items.add(new File([u], "image.jpg", {{ type: "image/jpeg" }}));
document.getElementById("i").files = t.files;
document.getElementById("f").submit();
</script></body></html>
"#,
        data = base64(jpeg),
    )
}

/// Removes hand-off pages left behind (e.g. if Nook quit within a minute).
fn sweep(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().is_ok_and(|age| age > KEEP_PAGE));
        if name.to_string_lossy().starts_with(PAGE_PREFIX) && old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn search(frame: &Frame) -> nook_platform::Result<PathBuf> {
    let jpeg = frame.encode_jpeg(MAX_EDGE, QUALITY)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let dir = std::env::temp_dir();
    sweep(&dir);
    let path = dir.join(format!("{PAGE_PREFIX}{stamp}.html"));
    std::fs::write(&path, page(&jpeg, stamp))
        .map_err(|e| nook_platform::PlatformError::Other(format!("writing lens page: {e}")))?;
    shell::open(&path.to_string_lossy())?;
    Ok(path)
}

impl Lens {
    fn start(&mut self) {
        self.snipper.toggle(SnipMode::Region, HINT, |outcome| {
            let SnipOutcome::Region { frame, .. } = outcome else {
                return None;
            };
            match search(&frame) {
                Ok(path) => {
                    tracing::info!("lens: {}x{} sent to the browser", frame.width, frame.height);
                    // Give the browser time to read the page.
                    let spawned = std::thread::Builder::new()
                        .name("nook-lens-cleanup".into())
                        .spawn(move || {
                            std::thread::sleep(KEEP_PAGE);
                            let _ = std::fs::remove_file(path);
                        });
                    if let Err(e) = spawned {
                        tracing::debug!("lens cleanup thread: {e}");
                    }
                    Some(true)
                }
                Err(e) => {
                    tracing::warn!("lens: {e}");
                    Some(false)
                }
            }
        });
    }
}

impl Module for Lens {
    fn id(&self) -> &'static str {
        "lens"
    }

    fn name(&self) -> &str {
        "Lens search"
    }

    fn icon(&self) -> &'static str {
        icons::SCAN
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }

    fn attach(&mut self, ctx: &egui::Context) {
        self.snipper.attach(ctx);
    }

    fn is_active(&self) -> bool {
        self.snipper.busy()
    }

    fn overlay_open(&self) -> bool {
        self.snipper.busy()
    }

    fn on_tick(&mut self) {
        for ok in self.snipper.poll() {
            self.snipper.flash(ok);
        }
    }

    fn next_repaint(&self, _tile_visible: bool) -> Option<Duration> {
        self.snipper.next_repaint()
    }

    fn default_hotkey(&self) -> Option<&'static str> {
        Some("ctrl+alt+KeyL")
    }

    fn on_hotkey(&mut self) {
        self.start();
    }

    fn ui_tile(&mut self, ui: &mut egui::Ui, ctx: &mut TileCtx<'_>) {
        if self
            .snipper
            .tile(ui, ctx, icons::SCAN, "lens-tile")
            .on_hover_text("Search part of the screen with Google Lens (opens your browser)")
            .clicked()
        {
            self.start();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xFF, 0xFE, 0xFD]), "//79");
    }

    #[test]
    fn page_posts_the_image() {
        let html = page(b"foo", 42);
        assert!(
            html.contains(r#"action="https://lens.google.com/v3/upload?ep=ccm&amp;s=&amp;st=42""#)
        );
        assert!(html.contains(r#"name="encoded_image""#));
        assert!(html.contains(r#"<meta name="referrer" content="no-referrer">"#));
        assert!(html.contains(r#"atob("Zm9v")"#));
    }
}
