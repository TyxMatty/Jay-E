// The bridge to src-tauri/src/update.rs: Jay-E's latest GitHub release, downloaded and checked in Rust.
import { isTauri } from './files.js';

const core = () => import('@tauri-apps/api/core');

/** The newer version on GitHub ("0.2.0"), or null. */
export async function checkUpdate() {
  return isTauri ? (await core()).invoke('update_check') : null;
}

/** Downloads the installer and checks its SHA-256; `onPercent(0..100)` along the way. */
export async function downloadUpdate(onPercent) {
  const { invoke, Channel } = await core();
  const progress = new Channel();
  progress.onmessage = onPercent;
  await invoke('update_download', { onProgress: progress });
}

/** Runs the downloaded installer and quits; Jay-E opens again when it's done. */
export async function installUpdate() {
  return (await core()).invoke('update_install');
}
