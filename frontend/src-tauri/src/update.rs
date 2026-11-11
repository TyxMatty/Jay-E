//! Updating Jay-E from its GitHub releases with the line's shared updater (`jay-i`): the installer
//! comes only from this repository's latest release and runs only if its SHA-256 matches the digest
//! GitHub computed (docs/ARQUITETURA.md, section 7).

use std::path::PathBuf;
use std::sync::Mutex;
use tauri::ipc::Channel;
use tauri::{AppHandle, State};

const REPO: &str = "TyxMatty/Jay-E";
const AGENT: &str = concat!("Jay-E/", env!("CARGO_PKG_VERSION"));

/// The installer `update_download` checked, waiting for `update_install`.
#[derive(Default)]
pub struct Downloaded(Mutex<Option<PathBuf>>);

/// The version to offer (without the leading v), when the release is newer than `current`.
fn offer(release: &jay_i::Release, current: &str) -> Option<String> {
    jay_i::newer(&release.tag_name, current).then(|| release.tag_name.trim_start_matches(['v', 'V']).to_string())
}

/// The newer version on GitHub, or None (up to date, or no release yet).
#[tauri::command]
pub async fn update_check(app: AppHandle) -> Result<Option<String>, String> {
    let current = app.package_info().version.to_string();
    tauri::async_runtime::spawn_blocking(move || Ok(jay_i::latest(REPO, AGENT)?.and_then(|r| offer(&r, &current))))
        .await
        .map_err(|e| e.to_string())?
}

/// Downloads the latest installer into a new temporary folder and checks its SHA-256.
/// `on_progress` gets the percent. On any failure nothing is left behind.
#[tauri::command]
pub async fn update_download(on_progress: Channel<u64>, downloaded: State<'_, Downloaded>) -> Result<(), String> {
    let path = tauri::async_runtime::spawn_blocking(move || {
        let release = jay_i::latest(REPO, AGENT)?.ok_or("there is no release")?;
        let installer = jay_i::pick_installer(REPO, &release)?;
        let mut last = u64::MAX;
        jay_i::download(&installer, AGENT, |done, total| {
            let percent = done * 100 / total.max(1);
            if percent != last {
                last = percent;
                let _ = on_progress.send(percent);
            }
        })
    })
    .await
    .map_err(|e| e.to_string())??;
    *downloaded.0.lock().unwrap() = Some(path);
    Ok(())
}

/// Runs the checked installer and quits: /P shows only its progress bar, /UPDATE keeps the
/// settings, /R opens Jay-E again when it's done. The page asks about unsaved edits first.
#[tauri::command]
pub fn update_install(app: AppHandle, downloaded: State<'_, Downloaded>) -> Result<(), String> {
    let path = downloaded.0.lock().unwrap().take().ok_or("no update downloaded")?;
    // shortcut: the temp folder stays (the installer runs from it); Windows' temp cleanup takes it.
    jay_i::run(&path, jay_i::Mode::Update { relaunch: true }).spawn().map_err(|e| e.to_string())?;
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> jay_i::Release {
        jay_i::Release { tag_name: tag.into(), html_url: String::new(), assets: vec![] }
    }

    #[test]
    fn offers_only_a_newer_version() {
        assert_eq!(offer(&release("v0.2.3"), "0.2.2"), Some("0.2.3".into()));
        assert_eq!(offer(&release("V0.2.3"), "0.2.2"), Some("0.2.3".into()));
        assert_eq!(offer(&release("0.10.0"), "0.2.2"), Some("0.10.0".into()));
        assert_eq!(offer(&release("v0.2.2"), "0.2.2"), None);
        assert_eq!(offer(&release("v0.2.1"), "0.2.2"), None);
        assert_eq!(offer(&release("nightly"), "0.2.2"), None);
    }
}
