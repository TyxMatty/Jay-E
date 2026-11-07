//! Updating Jay-E from its GitHub releases, as Jay-H updates the Jay apps (Jay-H's
//! src-tauri/src/products.rs): the installer comes only from this repository's latest release and
//! runs only if its SHA-256 matches the `digest` GitHub computed (docs/ARQUITETURA.md, section 7).

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::ipc::Channel;
use tauri::{AppHandle, State};

const REPO: &str = "TyxMatty/Jay-E";

/// Far above Jay-E's installer (a few MB); a release asset bigger than this isn't ours.
const MAX_INSTALLER: u64 = 200 * 1024 * 1024;

/// The installer `update_download` checked, waiting for `update_install`.
#[derive(Default)]
pub struct Downloaded(Mutex<Option<PathBuf>>);

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    size: u64,
    browser_download_url: String,
    digest: Option<String>,
}

fn get(url: &str) -> Result<ureq::http::Response<ureq::Body>, String> {
    // GitHub's API refuses requests without a User-Agent.
    ureq::get(url).header("User-Agent", "Jay-E").call().map_err(|e| e.to_string())
}

/// The latest release, or None when the repository has none yet.
fn latest() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    match ureq::get(&url).header("User-Agent", "Jay-E").call() {
        Err(ureq::Error::StatusCode(404)) => Ok(None),
        Err(e) => Err(e.to_string()),
        Ok(mut response) => response.body_mut().read_json().map(Some).map_err(|e| e.to_string()),
    }
}

/// The release's version when it is newer than `current` ("v0.2.0" > 0.1.0).
fn newer(release: &Release, current: &Version) -> Option<Version> {
    Version::parse(release.tag_name.trim_start_matches('v')).ok().filter(|v| v > current)
}

/// The release's x64 installer and its SHA-256, after the same checks as Jay-H.
fn pick_installer(release: Release) -> Result<(Asset, String), String> {
    let asset = release
        .assets
        .into_iter()
        .find(|a| a.name.ends_with("_x64-setup.exe"))
        .ok_or("the latest release has no Windows installer")?;
    if !asset.browser_download_url.starts_with(&format!("https://github.com/{REPO}/releases/download/")) {
        return Err(format!("unexpected download address: {}", asset.browser_download_url));
    }
    if asset.name.contains(['/', '\\', ':']) || asset.size > MAX_INSTALLER {
        return Err(format!("unexpected installer: {} ({} bytes)", asset.name, asset.size));
    }
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("the installer has no SHA-256 on GitHub, so it can't be checked")?
        .to_ascii_lowercase();
    Ok((asset, sha256))
}

/// Copies `from` into `to` and returns its SHA-256 (lowercase hex) and length. Fails past `limit` bytes.
fn copy_hashed(mut from: impl Read, mut to: impl Write, limit: u64, mut progress: impl FnMut(u64)) -> io::Result<(String, u64)> {
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = from.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > limit {
            return Err(io::Error::other("the download is bigger than GitHub said"));
        }
        hasher.update(&buf[..n]);
        to.write_all(&buf[..n])?;
        progress(total);
    }
    to.flush()?;
    Ok((hasher.finalize().iter().map(|b| format!("{b:02x}")).collect(), total))
}

/// The newer version on GitHub, or None (up to date, or no release yet).
#[tauri::command]
pub async fn update_check(app: AppHandle) -> Result<Option<String>, String> {
    let current = app.package_info().version.clone();
    tauri::async_runtime::spawn_blocking(move || Ok(latest()?.and_then(|r| newer(&r, &current)).map(|v| v.to_string())))
        .await
        .map_err(|e| e.to_string())?
}

/// Downloads the latest installer into a new temporary folder and checks its SHA-256.
/// `on_progress` gets the percent. On any failure the folder is removed.
#[tauri::command]
pub async fn update_download(on_progress: Channel<u64>, downloaded: State<'_, Downloaded>) -> Result<(), String> {
    let path = tauri::async_runtime::spawn_blocking(move || {
        let release = latest()?.ok_or("there is no release")?;
        let (asset, sha256) = pick_installer(release)?;
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("jay-e-update-{}-{nanos}", std::process::id()));
        fs::create_dir(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(&asset.name);
        let result = (|| {
            let mut response = get(&asset.browser_download_url)?;
            // ureq stops bodies at 10 MB by default.
            let body = response.body_mut().with_config().limit(asset.size + 1).reader();
            let file = fs::File::create(&path).map_err(|e| e.to_string())?;
            let mut last = u64::MAX;
            let (got, len) = copy_hashed(body, file, asset.size, |done| {
                let percent = done * 100 / asset.size.max(1);
                if percent != last {
                    last = percent;
                    let _ = on_progress.send(percent);
                }
            })
            .map_err(|e| e.to_string())?;
            if len != asset.size || got != sha256 {
                return Err(format!("{} doesn't match GitHub's SHA-256; it was not run", asset.name));
            }
            Ok(path)
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&dir);
        }
        result
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
    Command::new(&path).args(["/P", "/UPDATE", "/R"]).spawn().map_err(|e| e.to_string())?;
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://github.com/TyxMatty/Jay-E/releases/download/v0.2.0/Jay-E_0.2.0_x64-setup.exe";
    const DIGEST: &str = "sha256:848005e8a2f42200b36f5553fd144a9d8aefdcbddbbf1ad49b56c65f473d63de";

    fn release(tag: &str, url: &str, digest: Option<&str>) -> Release {
        Release {
            tag_name: tag.into(),
            assets: vec![Asset { name: "Jay-E_0.2.0_x64-setup.exe".into(), size: 5_000_000, browser_download_url: url.into(), digest: digest.map(Into::into) }],
        }
    }

    #[test]
    fn newer_compares_versions() {
        let current = Version::new(0, 1, 0);
        assert_eq!(newer(&release("v0.2.0", URL, None), &current), Some(Version::new(0, 2, 0)));
        assert_eq!(newer(&release("0.10.0", URL, None), &current), Some(Version::new(0, 10, 0)));
        assert_eq!(newer(&release("v0.1.0", URL, None), &current), None);
        assert_eq!(newer(&release("v0.0.9", URL, None), &current), None);
        assert_eq!(newer(&release("nightly", URL, None), &current), None);
    }

    #[test]
    fn pick_installer_checks_address_and_digest() {
        let (asset, sha) = pick_installer(release("v0.2.0", URL, Some(DIGEST))).unwrap();
        assert_eq!((asset.name.as_str(), sha.as_str()), ("Jay-E_0.2.0_x64-setup.exe", &DIGEST[7..]));
        // Another repository's file, another host, no digest, a malformed digest, no installer.
        assert!(pick_installer(release("v0.2.0", &URL.replace("Jay-E/", "Jay-C/"), Some(DIGEST))).is_err());
        assert!(pick_installer(release("v0.2.0", &URL.replace("github.com", "example.com"), Some(DIGEST))).is_err());
        assert!(pick_installer(release("v0.2.0", URL, None)).is_err());
        assert!(pick_installer(release("v0.2.0", URL, Some("sha256:abc"))).is_err());
        assert!(pick_installer(Release { tag_name: "v0.2.0".into(), assets: vec![] }).is_err());
    }

    #[test]
    fn copy_hashed_hashes_and_limits() {
        let mut out = Vec::new();
        let mut seen = 0;
        let (sha, len) = copy_hashed(&b"abc"[..], &mut out, 3, |n| seen = n).unwrap();
        assert_eq!(sha, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!((len, seen, out.as_slice()), (3, 3, &b"abc"[..]));
        assert!(copy_hashed(&b"abcd"[..], Vec::new(), 3, |_| {}).is_err());
    }
}
