// The bridge to src-tauri/src/text.rs. Errors come back as { kind: 'changed' | 'unencodable' |
// 'toobig' | 'binary' | 'io', ch?, message? }.

const call = async (cmd, args) => (await import('@tauri-apps/api/core')).invoke(cmd, args);

/** { text, encoding, lineEnding, stamp }; `text` always has \n line endings. */
export const openText = (path) => call('text_open', { path });
export const fileStamp = (path) => call('text_stamp', { path });
/** Writes `file` back with its encoding and line endings; resolves to the new stamp. */
export const saveText = (path, file, force = false) => call('text_save', { path, file, force });
export const renderMarkdown = (path, text) => call('markdown_render', { path, text });
/** Resolves to the saved path, or null if the dialog was cancelled. */
export const exportHtml = (path, text, title) => call('markdown_export_html', { path, text, title });

export const sameStamp = (a, b) => a.len === b.len && a.modifiedUs === b.modifiedUs;
