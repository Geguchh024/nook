//! The desktop wallpaper: its dominant accent color, setting a new one, and
//! the folder picker the wallpaper browser uses.

use crate::{PlatformError, Result, Win32Context, system};
use std::path::{Path, PathBuf};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::UI::Shell::{
    DesktopWallpaper, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, IDesktopWallpaper,
    IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_SETDESKWALLPAPER, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SystemParametersInfoW,
};
use windows::core::{HSTRING, PCWSTR};

/// Sets `path` as the wallpaper on every monitor. Blocks while Windows
/// applies it (it fades it in), so call it off the UI thread.
pub fn set_desktop(path: &Path) -> Result<()> {
    let wide = HSTRING::from(path.as_os_str());
    // SAFETY: per-thread COM init (harmless if repeated); the path outlives
    // the calls.
    let modern = unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        CoCreateInstance::<_, IDesktopWallpaper>(&DesktopWallpaper, None, CLSCTX_ALL)
            .and_then(|w| w.SetWallpaper(PCWSTR::null(), PCWSTR(wide.as_ptr())))
    };
    if let Err(e) = modern {
        tracing::debug!("IDesktopWallpaper::SetWallpaper: {e}, falling back to SPI");
        // SAFETY: SPI_SETDESKWALLPAPER reads a null-terminated path.
        unsafe {
            SystemParametersInfoW(
                SPI_SETDESKWALLPAPER,
                0,
                Some(wide.as_ptr() as *mut _),
                SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
            )
        }
        .api("SystemParametersInfoW(SPI_SETDESKWALLPAPER)")?;
    }
    Ok(())
}

/// Shows the system folder picker, starting in `start`. Returns `None` if
/// it was cancelled. Modal; call it on its own thread.
pub fn pick_folder(title: &str, start: Option<&Path>) -> Result<Option<PathBuf>> {
    // SAFETY: COM calls on a dialog we own; the returned path string is
    // freed with CoTaskMemFree after copying.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL)
            .api("CoCreateInstance(FileOpenDialog)")?;
        let opts = dialog.GetOptions().api("IFileDialog::GetOptions")?;
        dialog
            .SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)
            .api("IFileDialog::SetOptions")?;
        let _ = dialog.SetTitle(&HSTRING::from(title));
        if let Some(dir) = start.filter(|d| d.is_dir()) {
            let wide = HSTRING::from(dir.as_os_str());
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&wide, None) {
                let _ = dialog.SetFolder(&item);
            }
        }
        if dialog.Show(None).is_err() {
            // Cancelled (or failed to show, which reads the same to the user).
            return Ok(None);
        }
        let item = dialog.GetResult().api("IFileDialog::GetResult")?;
        let name = item
            .GetDisplayName(SIGDN_FILESYSPATH)
            .api("IShellItem::GetDisplayName")?;
        let path = name.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(name.0 as *const _));
        Ok(path)
    }
}

/// Loads the wallpaper and returns a vivid accent derived from its dominant
/// hue. Runs on a background thread (image decode can take ~100 ms).
pub fn accent_from_wallpaper() -> Result<Option<[u8; 3]>> {
    let Some(path) = system::wallpaper_path() else {
        return Ok(None);
    };
    // Windows keeps a screen-sized copy of the current wallpaper. Decoding
    // that instead of the original keeps huge images (a 10000 px photo blew
    // the decoder's memory limit) cheap; the original is the fallback.
    let transcoded = std::env::var_os("APPDATA").map(|d| {
        PathBuf::from(d).join(r"Microsoft\Windows\Themes\TranscodedWallpaper")
    });
    let img = match transcoded.as_deref().map(decode) {
        Some(Ok(img)) => img,
        _ => decode(&path)
            .map_err(|e| PlatformError::Other(format!("decode {}: {e}", path.display())))?,
    };
    let thumb = img.thumbnail(64, 64).to_rgb8();
    let pixels: Vec<[u8; 3]> = thumb.pixels().map(|p| p.0).collect();
    Ok(dominant_accent(&pixels))
}

/// Decodes by content, not extension (the transcoded copy has none).
fn decode(path: &Path) -> std::result::Result<image::DynamicImage, image::ImageError> {
    image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()
}

/// Hue histogram weighted by saturation x value; the winning bin's average
/// color is pushed to a vivid, readable lightness. `None` for grayscale.
pub fn dominant_accent(pixels: &[[u8; 3]]) -> Option<[u8; 3]> {
    const BINS: usize = 24;
    let mut weight = [0f32; BINS];
    let mut sum = [[0f32; 3]; BINS];
    for &[r, g, b] in pixels {
        let (h, s, v) = rgb_to_hsv(r, g, b);
        let w = s * s * v;
        if w < 0.02 {
            continue;
        }
        let bin = ((h / 360.0) * BINS as f32) as usize % BINS;
        weight[bin] += w;
        sum[bin][0] += r as f32 * w;
        sum[bin][1] += g as f32 * w;
        sum[bin][2] += b as f32 * w;
    }
    let total: f32 = weight.iter().sum();
    let (best, &w) = weight
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))?;
    if w <= 0.0 || w < total * 0.08 || total < pixels.len() as f32 * 0.01 {
        return None;
    }
    let avg = [sum[best][0] / w, sum[best][1] / w, sum[best][2] / w];
    let (h, s, _) = rgb_to_hsv(avg[0] as u8, avg[1] as u8, avg[2] as u8);
    Some(hsv_to_rgb(h, s.clamp(0.55, 0.85), 0.95))
}

fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let s = if max == 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [to(r), to(g), to(b)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grayscale_has_no_accent() {
        let px = vec![[128, 128, 128]; 100];
        assert_eq!(dominant_accent(&px), None);
    }

    #[test]
    fn mostly_blue_gives_blue() {
        let mut px = vec![[20, 60, 200]; 80];
        px.extend(vec![[200, 40, 40]; 20]);
        let [r, g, b] = dominant_accent(&px).expect("accent");
        assert!(b > r && b > g, "{r},{g},{b}");
    }

    #[test]
    fn hsv_round_trip() {
        let (h, s, v) = rgb_to_hsv(124, 92, 255);
        let [r, g, b] = hsv_to_rgb(h, s, v);
        assert!((r as i32 - 124).abs() <= 1 && (g as i32 - 92).abs() <= 1 && b == 255);
    }
}
