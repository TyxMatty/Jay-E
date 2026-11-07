// The bridge to src-tauri/src/files.rs. Rust opens the files (dialog, drop, command line) and the
// page gets back { path, name, kind, format, size, error }; it never sends a path of its own.

export const isTauri = '__TAURI_INTERNALS__' in globalThis;

const core = () => import('@tauri-apps/api/core');

/** The open-file dialog. Resolves to the chosen files (empty when cancelled). */
export async function pickFiles() {
  return (await core()).invoke('pick_files');
}

/** Files passed on the command line of the first launch ("Open with"). */
export async function startupFiles() {
  return isTauri ? (await core()).invoke('startup_files') : [];
}

/** Files dropped on the window or sent by a second launch. */
export async function onFilesOpened(callback) {
  if (!isTauri) return;
  const { listen } = await import('@tauri-apps/api/event');
  await listen('files-opened', (event) => callback(event.payload));
}

/** `true` while files are dragged over the window, `false` when they leave or drop. */
export async function onDragOver(callback) {
  if (!isTauri) return;
  const { getCurrentWebview } = await import('@tauri-apps/api/webview');
  await getCurrentWebview().onDragDropEvent((event) => callback(event.payload.type === 'enter' || event.payload.type === 'over'));
}

/** Closing the window: `callback(event)` may call `event.preventDefault()` to keep it open. */
export async function onCloseRequested(callback) {
  if (!isTauri) return;
  const { getCurrentWindow } = await import('@tauri-apps/api/window');
  await getCurrentWindow().onCloseRequested(callback);
}

/** The URL the asset protocol serves the file at (Rust allowed only the opened ones). */
export async function fileUrl(file) {
  return (await core()).convertFileSrc(file.path);
}

/** "1.2 MB" */
export function formatSize(bytes) {
  const units = ['B', 'KB', 'MB', 'GB'];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${unit === 0 ? value : value.toFixed(1)} ${units[unit]}`;
}

/** A time in seconds as "1:02.500". */
export function clock(seconds) {
  if (!Number.isFinite(seconds) || seconds < 0) return '–';
  const m = Math.floor(seconds / 60);
  // Rounded first, so 59.9996 s is "1:00.000", not "0:60.000".
  const ms = Math.round((seconds - m * 60) * 1000);
  return ms >= 60_000 ? `${m + 1}:00.000` : `${m}:${(ms / 1000).toFixed(3).padStart(6, '0')}`;
}
