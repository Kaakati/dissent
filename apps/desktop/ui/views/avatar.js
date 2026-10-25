// Profile pictures and server icons. A picked file opens "Edit Image", where the picture is panned and zoomed under a
// mask. Apply draws the crop onto a 256 px canvas and encodes it, so no original bytes (EXIF included) leave the app.
// The result goes to the engine as base64; the engine stores it as an encrypted blob.
import * as actions from '../actions.js';
import { state } from '../state.js';
import { el, icon, avatar, initials, setHue, withImage, openDialog, toast } from '../ui.js';
import { imageOf } from '../avatars.js';

const TYPES = ['image/png', 'image/jpeg', 'image/webp', 'image/gif'];
const MAX_FILE = 10 * 1024 * 1024;
const MAX_OUTPUT = 256 * 1024;
const OUT = 256; // exported side, px
const MASK = 0.8; // mask side as a share of the stage; matches `.crop-mask { inset: 10% }`
const MAX_ZOOM = 5; // the closest zoom, as a multiple of the zoom that just covers the mask
const PAN_STEP = 8; // px per arrow key; Shift moves four times as far
const ZOOM_STEP = 1.1;

// Opens a file picker. `onApply(base64)` gets the finished 256 px picture, once the user applies a crop.
// shape: 'circle' for people, 'rounded' for servers.
export function chooseImage({ shape, onApply }) {
  const input = el('input', { type: 'file', accept: TYPES.join(','), hidden: true });
  input.addEventListener('change', () => {
    const file = input.files?.[0];
    input.remove();
    if (file) acceptFile(file, shape, onApply);
  });
  input.addEventListener('cancel', () => input.remove());
  document.body.append(input);
  input.click();
}

function acceptFile(file, shape, onApply) {
  if (!TYPES.includes(file.type)) {
    toast('Pick a PNG, JPEG, WebP or GIF image');
    return;
  }
  if (file.size > MAX_FILE) {
    toast('Pick an image up to 10 MB');
    return;
  }
  const url = URL.createObjectURL(file);
  const picture = new Image();
  picture.src = url;
  picture.decode().then(
    () => showEditor(picture, url, shape, onApply),
    () => {
      URL.revokeObjectURL(url);
      toast('That file could not be read as an image');
    },
  );
}

// ---- Edit Image ----

function showEditor(picture, url, shape, onApply) {
  // rot: quarter turns clockwise. scale: screen px per natural px. x, y: picture centre from stage centre, in stage px.
  const crop = { rot: 0, scale: 1, x: 0, y: 0 };
  const side = () => stage.clientWidth;
  const turned = () => crop.rot % 2 === 1;
  const naturalW = () => (turned() ? picture.naturalHeight : picture.naturalWidth);
  const naturalH = () => (turned() ? picture.naturalWidth : picture.naturalHeight);
  // The smallest scale at which the picture still covers the mask.
  const fit = () => (side() * MASK) / Math.min(naturalW(), naturalH());

  const stage = el('div', {
    class: 'crop-stage',
    tabindex: '0',
    role: 'application',
    'aria-label': 'Crop area. Arrow keys move the picture, plus and minus zoom, R rotates.',
  });
  const view = el('img', { src: url, alt: '' });
  stage.append(view, dimOverlay(shape), el('div', { class: shape === 'rounded' ? 'crop-mask rounded' : 'crop-mask' }));

  // Pan and zoom are clamped so the mask is always covered by the picture.
  const clamp = () => {
    const mask = side() * MASK;
    crop.scale = Math.min(fit() * MAX_ZOOM, Math.max(fit(), crop.scale));
    const maxX = Math.max(0, (naturalW() * crop.scale - mask) / 2);
    const maxY = Math.max(0, (naturalH() * crop.scale - mask) / 2);
    crop.x = Math.min(maxX, Math.max(-maxX, crop.x));
    crop.y = Math.min(maxY, Math.max(-maxY, crop.y));
  };
  const paint = () => {
    clamp();
    const s = side();
    Object.assign(view.style, {
      width: `${picture.naturalWidth}px`,
      height: `${picture.naturalHeight}px`,
      left: `${(s - picture.naturalWidth) / 2}px`,
      top: `${(s - picture.naturalHeight) / 2}px`,
      transform: `translate(${crop.x}px, ${crop.y}px) rotate(${crop.rot * 90}deg) scale(${crop.scale})`,
    });
    zoom.value = String(Math.round((Math.log(crop.scale / fit()) / Math.log(MAX_ZOOM)) * 1000));
  };
  const zoomBy = (factor) => {
    crop.scale *= factor;
    paint();
  };
  const rotate = () => {
    crop.rot = (crop.rot + 1) % 4;
    paint();
  };
  const pan = (dx, dy) => {
    crop.x += dx;
    crop.y += dy;
    paint();
  };

  // ---- pointer: drag to pan, wheel or two-finger pinch to zoom ----
  const pointers = new Map();
  let pinch = 0;
  const gap = () => {
    const [a, b] = [...pointers.values()];
    return Math.hypot(a.x - b.x, a.y - b.y);
  };
  stage.addEventListener('pointerdown', (e) => {
    stage.setPointerCapture(e.pointerId);
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    pinch = pointers.size === 2 ? gap() : 0;
  });
  stage.addEventListener('pointermove', (e) => {
    const prev = pointers.get(e.pointerId);
    if (!prev) return;
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (pointers.size === 1) {
      pan(e.clientX - prev.x, e.clientY - prev.y);
    } else if (pointers.size === 2) {
      const next = gap();
      if (pinch) zoomBy(next / pinch);
      pinch = next;
    }
  });
  const release = (e) => {
    pointers.delete(e.pointerId);
    pinch = 0;
  };
  stage.addEventListener('pointerup', release);
  stage.addEventListener('pointercancel', release);
  stage.addEventListener('wheel', (e) => {
    e.preventDefault();
    zoomBy(Math.exp(-e.deltaY * 0.0015));
  }, { passive: false });

  const zoom = el('input', {
    type: 'range',
    min: '0',
    max: '1000',
    value: '0',
    'aria-label': 'Zoom',
    oninput: (e) => {
      crop.scale = fit() * Math.pow(MAX_ZOOM, Number(e.target.value) / 1000);
      paint();
    },
  });
  const rotateBtn = el('button', { class: 'icon-btn', icon: 'rotate-cw', 'aria-label': 'Rotate 90°', title: 'Rotate 90°', onclick: rotate });
  const large = icon('image');
  large.classList.add('lg');
  const controls = el('div', { class: 'crop-controls' }, icon('image'), zoom, large, rotateBtn);

  // Arrows pan (the slider keeps its own arrow keys), + and - zoom, R rotates, Enter applies. Esc closes the dialog.
  const onKey = (e) => {
    if (e.key === 'Enter' && e.target.tagName !== 'BUTTON') {
      e.preventDefault();
      apply();
      return;
    }
    const step = e.shiftKey ? PAN_STEP * 4 : PAN_STEP;
    const keys = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] };
    if (keys[e.key] && e.target !== zoom) {
      e.preventDefault();
      pan(...keys[e.key]);
    } else if (e.key === '+' || e.key === '=') {
      zoomBy(ZOOM_STEP);
    } else if (e.key === '-' || e.key === '_') {
      zoomBy(1 / ZOOM_STEP);
    } else if (e.key === 'r' || e.key === 'R') {
      rotate();
    }
  };

  const resize = new ResizeObserver(() => paint());
  resize.observe(stage);

  // ---- export: draw the crop at 256 px, then encode WebP at 0.85 with PNG as the fallback ----
  const encode = (type, quality) => new Promise((resolve) => {
    const canvas = document.createElement('canvas');
    canvas.width = OUT;
    canvas.height = OUT;
    const ctx = canvas.getContext('2d');
    const k = OUT / (side() * MASK);
    ctx.imageSmoothingQuality = 'high';
    // The picture's centre sits (x, y) from the mask centre, which is the canvas centre.
    ctx.translate(OUT / 2 + crop.x * k, OUT / 2 + crop.y * k);
    ctx.rotate((crop.rot * Math.PI) / 2);
    ctx.scale(crop.scale * k, crop.scale * k);
    ctx.drawImage(picture, -picture.naturalWidth / 2, -picture.naturalHeight / 2);
    canvas.toBlob(resolve, type, quality);
  });
  const exportCrop = async () => {
    const webp = await encode('image/webp', 0.85);
    return webp?.type === 'image/webp' ? webp : encode('image/png');
  };

  let close = () => {};
  const apply = async () => {
    const blob = await exportCrop();
    if (!blob) return;
    if (blob.size > MAX_OUTPUT) {
      toast('This crop is over 256 KB after compression. Try a simpler picture or a tighter crop.');
      return;
    }
    close();
    await onApply(base64(await blob.arrayBuffer()));
  };

  const applyBtn = el('button', { class: 'primary', text: 'Apply', onclick: apply });
  const resetBtn = el('button', {
    class: 'link-btn',
    text: 'Reset',
    onclick: () => {
      crop.rot = 0;
      crop.x = 0;
      crop.y = 0;
      crop.scale = fit();
      paint();
    },
  });
  const cancelBtn = el('button', { class: 'ghost', text: 'Cancel', onclick: () => close() });

  const wrap = el('div', { class: 'avatar-editor', onkeydown: onKey }, stage, controls);
  ({ close } = openDialog({
    title: 'Edit Image',
    content: wrap,
    footer: [resetBtn, cancelBtn, applyBtn],
    className: 'avatar-dialog',
    onClose: () => {
      resize.disconnect();
      URL.revokeObjectURL(url);
    },
  }));
  crop.scale = fit();
  paint();
  stage.focus();
}

// Dims everything outside the mask: a 60% black layer with the mask cut out (even-odd). Drawn in a 100 x 100 box that
// matches the stage. The mask is 80% of the stage, from 10 to 90; a server icon's corners have a radius of 33% of it.
function dimOverlay(shape) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('class', 'crop-dim');
  svg.setAttribute('viewBox', '0 0 100 100');
  svg.setAttribute('preserveAspectRatio', 'none');
  svg.setAttribute('aria-hidden', 'true');
  const r = 80 * 0.33;
  const hole = shape === 'rounded'
    ? `M${10 + r} 10H${90 - r}A${r} ${r} 0 0 1 90 ${10 + r}V${90 - r}A${r} ${r} 0 0 1 ${90 - r} 90H${10 + r}A${r} ${r} 0 0 1 10 ${90 - r}V${10 + r}A${r} ${r} 0 0 1 ${10 + r} 10Z`
    : 'M10 50a40 40 0 1 0 80 0a40 40 0 1 0-80 0Z';
  const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
  path.setAttribute('d', `M0 0H100V100H0Z ${hole}`);
  path.setAttribute('fill-rule', 'evenodd');
  svg.append(path);
  return svg;
}

function base64(buffer) {
  const bytes = new Uint8Array(buffer);
  let text = '';
  for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(text);
}

// ---- entry points ----

// A picture with "change" and "remove" beside it. `current()` draws the picture or the initials circle; `hasPicture()`
// says whether "remove" applies. onChange(bytes) and onRemove() resolve true on success.
function pictureControl({ current, hasPicture, shape, labels, onChange, onRemove }) {
  const host = el('div', { class: 'avatar-setting' });
  const paint = () => {
    const change = el('button', {
      class: 'ghost',
      icon: 'image',
      text: labels.change,
      onclick: () => chooseImage({ shape, onApply: async (bytes) => { if (await onChange(bytes)) paint(); } }),
    });
    const remove = el('button', {
      class: 'danger-text',
      icon: 'trash-2',
      text: labels.remove,
      onclick: async () => { if (await onRemove()) paint(); },
    });
    host.replaceChildren(current(), el('div', { class: 'row' }, change, hasPicture() ? remove : null));
  };
  paint();
  return host;
}

// Settings › My identity: the avatar shown everywhere the user appears.
export function avatarSection() {
  const me = () => state.me;
  return el('section', { class: 'settings-block' },
    el('h2', { text: 'Avatar' }),
    pictureControl({
      current: () => avatar(me()?.display_name ?? '?', me()?.root_id, 'xl'),
      hasPicture: () => imageOf(me()?.root_id) !== null,
      shape: 'circle',
      labels: { change: 'Change avatar', remove: 'Remove avatar' },
      onChange: actions.setAvatar,
      onRemove: actions.clearAvatar,
    }));
}

// Server menu › Server icon (owners and admins). The icon shows on the server rail, in the switcher and in invites.
export function openServerIconDialog(community) {
  const current = () => withImage(setHue(el('span', { class: 'mini-server xl', text: initials(community.name) }), community.id), community.id);
  openDialog({
    title: 'Server icon',
    sub: community.name,
    content: pictureControl({
      current,
      hasPicture: () => imageOf(community.id) !== null,
      shape: 'rounded',
      labels: { change: 'Change icon', remove: 'Remove icon' },
      onChange: (bytes) => actions.setCommunityIcon(community.id, bytes),
      onRemove: () => actions.clearCommunityIcon(community.id),
    }),
  });
}
