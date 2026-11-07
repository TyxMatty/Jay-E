// The top bar's "Update" button: hidden until GitHub has a newer Jay-E (checked once at startup).
import { h } from './dom.js';
import { confirmQuit } from './files.js';
import { checkUpdate, downloadUpdate, installUpdate } from '../core/update.js';
import { t } from '../core/i18n.js';

export function createUpdateButton() {
  const button = h('button', { type: 'button', class: 'btn', hidden: true });
  let downloaded = false;

  const show = (label, title = '') => {
    button.textContent = label;
    button.title = title;
  };

  button.addEventListener('click', async () => {
    button.disabled = true;
    try {
      if (!downloaded) {
        await downloadUpdate((percent) => show(t('Downloading {percent}%', { percent })));
        downloaded = true;
      }
      // Asked after the download, so nothing edited meanwhile is lost.
      if (confirmQuit()) return await installUpdate();
      show(t('Restart to update'));
    } catch (error) {
      show(t('Update failed, try again'), String(error));
    }
    button.disabled = false;
  });

  // Offline or GitHub down: no button, nothing to say.
  checkUpdate()
    .then((version) => {
      if (!version) return;
      show(t('Update to {version}', { version }), t('Jay-E {version} is out', { version }));
      button.hidden = false;
    })
    .catch(() => {});
  return button;
}
