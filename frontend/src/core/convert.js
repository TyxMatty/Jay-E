// The batch converter's bridge to src-tauri/src/convert.rs, and what each kind of file can become
// (the same list as convert.rs `can_become`).

const core = () => import('@tauri-apps/api/core');
const call = async (cmd, args) => (await core()).invoke(cmd, args);

/** The groups the screen sets a format for, with the outputs each can become ([extension, name]). */
export const GROUPS = [
  { key: 'image', kinds: ['image'], name: 'Images', to: [['webp', 'WebP'], ['avif', 'AVIF'], ['jpg', 'JPEG'], ['png', 'PNG'], ['gif', 'GIF'], ['bmp', 'BMP'], ['tif', 'TIFF'], ['ico', 'ICO'], ['qoi', 'QOI']] },
  { key: 'document', kinds: ['document', 'markdown'], name: 'Documents', to: [['docx', 'DOCX'], ['md', 'Markdown'], ['html', 'HTML'], ['txt', 'TXT']] },
  { key: 'pdf', kinds: ['pdf'], name: 'PDF', to: [['txt', 'TXT']] },
  { key: 'video', kinds: ['video'], name: 'Video', to: [['mp4', 'MP4'], ['m4a', 'M4A (sound only)']] },
  { key: 'audio', kinds: ['audio'], name: 'Audio', to: [['m4a', 'M4A']] },
];

/** The group a file belongs to, or null if Jay-E doesn't convert its kind (LaTeX, text, unknown). */
export const groupOf = (file) => (file.error ? null : (GROUPS.find((g) => g.kinds.includes(file.kind)) ?? null));

/** The output extension for `file`: its group's choice (the first format if none yet), or null. */
export function outputOf(file, choices) {
  const group = groupOf(file);
  return group ? (choices[group.key] ?? group.to[0][0]) : null;
}

/** The folder dialog: the folder shown, or null if cancelled. */
export const pickFolder = () => call('convert_pick_folder');
/** For each item ({ path, to }), whether its output exists in the folder. */
export const checkExisting = (items) => call('convert_check', { items });
export const cancelBatch = () => call('convert_cancel');

/**
 * Starts the batch. `existing`: 'replace' | 'rename' | 'skip'. Events: started, progress, done,
 * skipped, failed ({ index, error }), finished ({ cancelled }).
 */
export async function startBatch(items, existing, quality, onEvent) {
  const { Channel } = await core();
  const channel = new Channel();
  channel.onmessage = onEvent;
  return call('convert_start', { items, existing, quality, channel });
}

/** An error from any converter as one line ({ kind, message?, what?, width?, height? }). */
export function describeError(error, t) {
  if (!error) return '';
  if (error.message || error.what) return error.message ?? error.what;
  return (
    {
      toobig: t('The file is too big.'),
      unsupported: t('This kind of file can’t become that format.'),
      invalid: t('The file is damaged or isn’t what its name says.'),
      password: t('The PDF has a password.'),
    }[error.kind] ??
    error.kind ??
    String(error)
  );
}
