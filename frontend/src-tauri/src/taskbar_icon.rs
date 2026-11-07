//! The taskbar icon, drawn by the page in the current style (src/ui/window-icon.js). From Jay-C
//! (src-tauri/src/taskbar_icon.rs), without its picture-in-picture grouping.
//!
//! `ICON_BIG` alone isn't enough once the app is installed: Windows ties `jay-e.exe` to its Start
//! menu shortcut and shows the shortcut's icon, ignoring the window's. So the window also gets its
//! own AppUserModelID, which unties it, and "relaunch" properties: the taskbar (and a pin made from
//! it) then use the relaunch icon, an .ico of the same picture. The taskbar only reads it again when
//! the ID changes, and a clear and a set in the same instant don't count (Jay-C measured it), so
//! each update clears the ID and sets it back a moment later.
//!
//! Where the .ico is matters: Explorer reads it by path. A process started from inside another
//! app's MSIX package (a dev build started by a packaged desktop app, say) has its AppData writes
//! redirected into that package's private copy, `%LOCALAPPDATA%\Packages\<package>\LocalCache\Local\…`,
//! even without the package's identity (measured: the check for one said no). Explorer, outside,
//! finds no file at the AppData path and shows a blank icon (Jay-A hit this). So after writing, the
//! .ico is looked for in those private copies, and the taskbar is given the real file.

use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use tauri::Manager;
use windows::Win32::Foundation::{HWND, LPARAM, PROPERTYKEY, WPARAM};
use windows::Win32::Storage::EnhancedStorage::{PKEY_AppUserModel_ID, PKEY_AppUserModel_RelaunchCommand, PKEY_AppUserModel_RelaunchDisplayNameResource, PKEY_AppUserModel_RelaunchIconResource};
use windows::Win32::System::Com::CoTaskMemAlloc;
use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::WindowsAndMessaging::{CreateIcon, DestroyIcon, HICON, ICON_BIG, SendMessageW, WM_SETICON};
use windows::core::PWSTR;

/// The window's own app ID: not the Start menu shortcut's (`io.github.tyxmatty.jaye`).
const WINDOW_APP_ID: &str = "io.github.tyxmatty.jaye.window";

/// Where Explorer can find a file this process wrote under `%LOCALAPPDATA%`: the same path, or
/// a package's private copy of it when the write was redirected (see above).
fn real_path(written: &Path) -> PathBuf {
    let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) else { return written.to_path_buf() };
    let Ok(rest) = written.strip_prefix(&local) else { return written.to_path_buf() };
    let Ok(packages) = std::fs::read_dir(local.join("Packages")) else { return written.to_path_buf() };
    packages.flatten().map(|p| p.path().join("LocalCache").join("Local").join(rest)).find(|p| p.is_file()).unwrap_or_else(|| written.to_path_buf())
}

/// `rgba`: `size` x `size` pixels, 4 bytes each (straight alpha), as from a canvas.
#[tauri::command]
pub fn set_taskbar_icon(window: tauri::WebviewWindow, rgba: Vec<u8>, size: u32) -> Result<(), String> {
    let pixels = (size * size) as usize;
    if size == 0 || size > 256 || rgba.len() != pixels * 4 {
        return Err(format!("expected {size}x{size} RGBA pixels, got {} bytes", rgba.len()));
    }
    let hwnd = HWND(window.hwnd().map_err(|e| e.to_string())?.0);
    // Named after its content, so a changed picture is never served from an icon cache.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    rgba.hash(&mut hasher);
    let dir = window.app_handle().path().app_local_data_dir().map_err(|e| e.to_string())?;
    let ico_path = dir.join(format!("window-icon-{:016x}.ico", hasher.finish()));
    if !ico_path.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(&ico_path, ico(&rgba, size)).map_err(|e| e.to_string())?;
    }
    // CreateIcon takes BGRA color bits and an AND mask (here one byte per pixel: the inverted alpha).
    let mut bgra = rgba;
    let mut and_mask = Vec::with_capacity(pixels);
    for px in bgra.chunks_exact_mut(4) {
        and_mask.push(px[3].wrapping_sub(u8::MAX));
        px.swap(0, 2);
    }
    unsafe {
        let icon = CreateIcon(None, size as i32, size as i32, 1, 32, and_mask.as_ptr(), bgra.as_ptr()).map_err(|e| e.to_string())?;
        // The previous big icon is one this command made (nothing else sets ICON_BIG): free it.
        let old = SendMessageW(hwnd, WM_SETICON, Some(WPARAM(ICON_BIG as usize)), Some(LPARAM(icon.0 as isize)));
        if old.0 != 0 {
            let _ = DestroyIcon(HICON(old.0 as _));
        }
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let ico_path = real_path(&ico_path);
    relaunch_with_icon(hwnd, &exe, &ico_path).map_err(|e| format!("taskbar properties: {e}"))?;
    let raw = hwnd.0 as isize; // HWND isn't Send
    let main = window.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let _ = main.run_on_main_thread(move || {
            if let Err(e) = set_app_id(HWND(raw as _)) {
                eprintln!("taskbar app ID: {e}");
            }
        });
    });
    Ok(())
}

/// The relaunch properties, and the app ID cleared (set again by `set_app_id`).
fn relaunch_with_icon(hwnd: HWND, exe: &Path, ico_path: &Path) -> windows::core::Result<()> {
    let store: IPropertyStore = unsafe { SHGetPropertyStoreForWindow(hwnd)? };
    set_text(&store, &PKEY_AppUserModel_RelaunchCommand, &format!("\"{}\"", exe.display()))?;
    set_text(&store, &PKEY_AppUserModel_RelaunchDisplayNameResource, "Jay-E")?;
    set_text(&store, &PKEY_AppUserModel_RelaunchIconResource, &ico_path.display().to_string())?;
    unsafe {
        store.SetValue(&PKEY_AppUserModel_ID, &PROPVARIANT::default())?;
        store.Commit()
    }
}

fn set_app_id(hwnd: HWND) -> windows::core::Result<()> {
    let store: IPropertyStore = unsafe { SHGetPropertyStoreForWindow(hwnd)? };
    set_text(&store, &PKEY_AppUserModel_ID, WINDOW_APP_ID)?;
    unsafe { store.Commit() }
}

fn set_text(store: &IPropertyStore, key: &PROPERTYKEY, text: &str) -> windows::core::Result<()> {
    let mut value = string_value(text);
    let result = unsafe { store.SetValue(key, &value) };
    let _ = unsafe { PropVariantClear(&mut value) };
    result
}

/// A VT_LPWSTR value (its string is freed by PropVariantClear).
fn string_value(text: &str) -> PROPVARIANT {
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let mut value = PROPVARIANT::default();
    unsafe {
        let ptr = CoTaskMemAlloc(wide.len() * 2) as *mut u16;
        ptr.copy_from_nonoverlapping(wide.as_ptr(), wide.len());
        let inner = &mut *value.Anonymous.Anonymous;
        inner.vt = VT_LPWSTR;
        inner.Anonymous.pwszVal = PWSTR(ptr);
    }
    value
}

/// A one-picture .ico (32-bit BGRA bitmap, bottom-up, empty AND mask) from straight-alpha RGBA.
fn ico(rgba: &[u8], size: u32) -> Vec<u8> {
    let side = size as usize;
    let mask_row = side.div_ceil(32) * 4; // 1 bit per pixel, rows padded to 32 bits
    let image_len = 40 + rgba.len() + mask_row * side;
    let dim = if size >= 256 { 0 } else { size as u8 }; // 0 means 256
    let mut out = Vec::with_capacity(22 + image_len);
    out.extend_from_slice(&[0, 0, 1, 0, 1, 0]); // ICONDIR: an icon, 1 picture
    out.extend_from_slice(&[dim, dim, 0, 0]); // ICONDIRENTRY: size, no palette
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
    out.extend_from_slice(&(image_len as u32).to_le_bytes());
    out.extend_from_slice(&22u32.to_le_bytes()); // where the picture starts
    out.extend_from_slice(&40u32.to_le_bytes()); // BITMAPINFOHEADER
    out.extend_from_slice(&(size as i32).to_le_bytes());
    out.extend_from_slice(&(2 * size as i32).to_le_bytes()); // color + mask
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]); // no compression, sizes and palette unused
    for row in rgba.chunks_exact(side * 4).rev() {
        for px in row.chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
        }
    }
    out.resize(22 + image_len, 0); // the AND mask: transparency comes from the alpha
    out
}

#[cfg(test)]
mod tests {
    use super::{ico, real_path};

    #[test]
    fn ico_layout() {
        // 2x2: top row red, green; bottom row blue, transparent.
        let rgba = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 0, 0, 0, 0];
        let file = ico(&rgba, 2);
        assert_eq!(&file[..6], &[0, 0, 1, 0, 1, 0]);
        assert_eq!((file[6], file[7]), (2, 2));
        let image_len = u32::from_le_bytes(file[14..18].try_into().unwrap()) as usize;
        assert_eq!(file.len(), 22 + image_len);
        assert_eq!(image_len, 40 + 16 + 4 * 2, "header, pixels, mask rows of 4 bytes");
        assert_eq!(i32::from_le_bytes(file[30..34].try_into().unwrap()), 4, "height counts the mask");
        // Bottom-up BGRA: the bottom row (blue, transparent) comes first.
        assert_eq!(&file[62..70], &[255, 0, 0, 255, 0, 0, 0, 0]);
        assert_eq!(&file[70..78], &[0, 0, 255, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn a_redirected_file_is_found_in_its_package_copy() {
        let local = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap());
        let rest = format!("jaye-test-{}/icon.ico", std::process::id());
        let copy = local.join("Packages").join(format!("JayeTest_{}", std::process::id())).join("LocalCache").join("Local").join(&rest);
        std::fs::create_dir_all(copy.parent().unwrap()).unwrap();
        std::fs::write(&copy, b"x").unwrap();
        let written = local.join(&rest);
        let found = real_path(&written);
        std::fs::remove_dir_all(local.join("Packages").join(format!("JayeTest_{}", std::process::id()))).unwrap();
        // Run inside a package's redirection, the test's own writes would be redirected too; the
        // copy it made by hand is what a redirected write looks like from outside.
        assert_eq!(found, copy);
        // Nothing redirected: the path itself.
        assert_eq!(real_path(&local.join("jaye-test-none/icon.ico")), local.join("jaye-test-none/icon.ico"));
    }
}
