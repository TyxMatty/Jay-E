/**
 * Tiny element builder: h('button', { class: 'btn', onClick }, 'Label').
 * Text children are inserted as text nodes (never parsed as HTML).
 * `html` is only for trusted static markup such as the icons in icons.js.
 */
export function h(tag, props = {}, ...children) {
  const el = document.createElement(tag);

  for (const [key, value] of Object.entries(props ?? {})) {
    if (value == null || value === false) continue;
    if (key.startsWith('on') && typeof value === 'function') {
      el.addEventListener(key.slice(2).toLowerCase(), value);
    } else if (key === 'class') {
      el.className = value;
    } else if (key === 'dataset') {
      Object.assign(el.dataset, value);
    } else if (key === 'html') {
      el.innerHTML = value;
    } else if (key === 'value' || key === 'checked' || key === 'muted' || key === 'srcObject') {
      // Properties, not attributes (e.g. the `muted` attribute doesn't mute a created <video>).
      el[key] = value;
    } else {
      el.setAttribute(key, value === true ? '' : String(value));
    }
  }

  for (const child of children.flat()) {
    if (child == null || child === false) continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return el;
}
