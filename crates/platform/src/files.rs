//! Files for the shelf: dragging them out to other apps, and the
//! thumbnails Explorer would show for them.
//!
//! Dragging out uses the shell's own data object for the files
//! (`IShellItemArray` bound to `BHID_DataObject`, which carries `CF_HDROP`,
//! shell ID lists and file contents) and `SHDoDragDrop` with the shell's
//! default drop source, so targets treat it exactly like a drag from
//! Explorer, drag image included. It must run on a thread where OLE is
//! initialized and the mouse button is down; it returns when the drop
//! happens or is cancelled.

use crate::{PlatformError, Result, Win32Context};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDIBits, GetObjectW,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, IDataObject};
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, OleInitialize,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, ILCreateFromPathW, ILFree, IShellItemArray, IShellItemImageFactory,
    SHCreateItemFromParsingName, SHCreateShellItemArrayFromIDLists, SHDoDragDrop,
    SIIGBF_BIGGERSIZEOK, SIIGBF_RESIZETOFIT,
};
use windows::core::{HSTRING, PCWSTR};

/// What the drop target did with the files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropResult {
    Copied,
    Moved,
    Linked,
    Cancelled,
}

/// Drags `paths` out of Nook. Call from the UI thread while the mouse
/// button is held; blocks until the drop.
pub fn drag_out(paths: &[PathBuf]) -> Result<DropResult> {
    if paths.is_empty() {
        return Ok(DropResult::Cancelled);
    }
    // SAFETY: OLE init is reference-counted and harmless when already done
    // (winit initializes it for its drop target).
    let _ = unsafe { OleInitialize(None) };
    let pidls: Vec<*mut ITEMIDLIST> = paths
        .iter()
        .filter_map(|p| {
            let wide = HSTRING::from(p.as_os_str());
            // SAFETY: a null-terminated path; the PIDL is freed below.
            let pidl = unsafe { ILCreateFromPathW(PCWSTR(wide.as_ptr())) };
            (!pidl.is_null()).then_some(pidl)
        })
        .collect();
    let result = (|| {
        if pidls.is_empty() {
            return Err(PlatformError::Other("none of the files exist".into()));
        }
        let list: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| p.cast_const()).collect();
        // SAFETY: valid PIDLs for the duration of the call.
        let items: IShellItemArray = unsafe { SHCreateShellItemArrayFromIDLists(&list) }
            .api("SHCreateShellItemArrayFromIDLists")?;
        // SAFETY: plain COM calls on the array we own.
        let data: IDataObject = unsafe { items.BindToHandler(None, &BHID_DataObject) }
            .api("IShellItemArray::BindToHandler(DataObject)")?;
        let allowed = DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK;
        // SAFETY: modal drag loop with the shell's default drop source.
        let effect: DROPEFFECT =
            unsafe { SHDoDragDrop(None, &data, None, allowed) }.api("SHDoDragDrop")?;
        Ok(if effect.0 & DROPEFFECT_MOVE.0 != 0 {
            DropResult::Moved
        } else if effect.0 & DROPEFFECT_COPY.0 != 0 {
            DropResult::Copied
        } else if effect.0 & DROPEFFECT_LINK.0 != 0 {
            DropResult::Linked
        } else {
            DropResult::Cancelled
        })
    })();
    for p in pidls {
        // SAFETY: each PIDL came from ILCreateFromPathW above.
        unsafe { ILFree(Some(p.cast_const())) };
    }
    result
}

/// Readies COM on a worker thread that makes thumbnails.
pub fn init_thumbnail_thread() {
    // SAFETY: per-thread COM init; repeated calls are harmless.
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
}

/// A thumbnail at most `size` pixels on each side: the picture for images,
/// videos and documents with previews, the file type's icon otherwise.
/// Returns `(width, height, premultiplied RGBA)`. Call on a thread set up
/// with [`init_thumbnail_thread`].
pub fn thumbnail(path: &Path, size: i32) -> Result<(u32, u32, Vec<u8>)> {
    let wide = HSTRING::from(path.as_os_str());
    // SAFETY: COM calls on a factory we own; the bitmap is freed below and
    // `GetDIBits` writes exactly `w * h` 32-bit pixels.
    unsafe {
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)
                .api("SHCreateItemFromParsingName")?;
        let bitmap = factory
            .GetImage(
                SIZE { cx: size, cy: size },
                SIIGBF_RESIZETOFIT | SIIGBF_BIGGERSIZEOK,
            )
            .api("IShellItemImageFactory::GetImage")?;
        let mut info = BITMAP::default();
        GetObjectW(
            bitmap.into(),
            std::mem::size_of::<BITMAP>() as i32,
            Some((&mut info as *mut BITMAP).cast()),
        );
        let (w, h) = (info.bmWidth, info.bmHeight.abs());
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; (w * h * 4).max(0) as usize];
        let dc = CreateCompatibleDC(None);
        let lines = GetDIBits(
            dc,
            bitmap,
            0,
            h as u32,
            Some(bgra.as_mut_ptr().cast()),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        let _ = DeleteDC(dc);
        let _ = DeleteObject(bitmap.into());
        if lines != h || w <= 0 {
            return Err(PlatformError::Other("GetDIBits(thumbnail) failed".into()));
        }
        // Bitmaps without alpha (some photo thumbnails) come back all zero.
        let opaque = info.bmBitsPixel < 32 || bgra.as_chunks::<4>().0.iter().all(|p| p[3] == 0);
        let rgba = bgra
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[2], p[1], p[0], if opaque { 255 } else { p[3] }])
            .collect();
        Ok((w as u32, h as u32, rgba))
    }
}

/// Shows the system file picker with multi-select. Returns no paths if it
/// was cancelled. Modal; call it on its own thread.
pub fn pick_files(title: &str) -> Result<Vec<PathBuf>> {
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};
    use windows::Win32::UI::Shell::{
        FOS_ALLOWMULTISELECT, FOS_FORCEFILESYSTEM, FileOpenDialog, IFileOpenDialog,
        SIGDN_FILESYSPATH,
    };
    // SAFETY: COM calls on a dialog we own; each path string is freed with
    // CoTaskMemFree after copying.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL)
            .api("CoCreateInstance(FileOpenDialog)")?;
        let opts = dialog.GetOptions().api("IFileDialog::GetOptions")?;
        dialog
            .SetOptions(opts | FOS_ALLOWMULTISELECT | FOS_FORCEFILESYSTEM)
            .api("IFileDialog::SetOptions")?;
        let _ = dialog.SetTitle(&HSTRING::from(title));
        if dialog.Show(None).is_err() {
            return Ok(Vec::new());
        }
        let items = dialog.GetResults().api("IFileOpenDialog::GetResults")?;
        let count = items.GetCount().api("IShellItemArray::GetCount")?;
        let mut paths = Vec::with_capacity(count as usize);
        for i in 0..count {
            let Ok(item) = items.GetItemAt(i) else {
                continue;
            };
            if let Ok(name) = item.GetDisplayName(SIGDN_FILESYSPATH) {
                if let Ok(s) = name.to_string() {
                    paths.push(PathBuf::from(s));
                }
                CoTaskMemFree(Some(name.0 as *const _));
            }
        }
        Ok(paths)
    }
}
