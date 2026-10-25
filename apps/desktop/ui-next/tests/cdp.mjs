// Minimal CDP helper for headless Edge (the drivers' only browser dependency): launch, evaluate, real input events, screenshots.
import { spawn } from 'node:child_process';
import { mkdirSync, rmSync, writeFileSync } from 'node:fs';

// The drivers run a headless Chromium-based browser. CHROME_PATH (or EDGE) names it; without either, Edge on Windows and
// google-chrome on Linux (the CI runners have it).
const BROWSER = process.env.CHROME_PATH ?? process.env.EDGE ?? (process.platform === 'win32'
  ? 'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe'
  : 'google-chrome');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function launch({ url, width = 1600, height = 1000, port = 9778, profile }) {
  rmSync(profile, { recursive: true, force: true });
  mkdirSync(profile, { recursive: true });
  // Linux runners start Chrome as a non-root user without a sandbox setup, so the sandbox is switched off there.
  const sandbox = process.platform === 'linux' ? ['--no-sandbox'] : [];
  // The app is a desktop app with a mouse. Headless Linux Chrome reports no hover or fine pointer, which hides the hover-only
  // controls (Tailwind wraps hover styles in @media (hover: hover)). primaryPointerType=4 is a fine pointer and
  // primaryHoverType=2 is hover; Windows Chrome already reports both, so this only changes the Linux runs.
  const pointer = ['--blink-settings=primaryPointerType=4,primaryHoverType=2,availablePointerTypes=4,availableHoverTypes=2'];
  const browser = spawn(BROWSER, [
    ...sandbox, ...pointer, '--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`,
    '--no-first-run', '--disable-gpu', '--hide-scrollbars', `--window-size=${width},${height}`, '--force-device-scale-factor=1', url,
  ], { stdio: 'ignore' });

  let target = null;
  for (let i = 0; i < 150 && !target; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      target = list.find((t) => t.type === 'page' && t.url.startsWith(url.slice(0, 22)));
    } catch { /* starting */ }
    if (!target) await sleep(400);
  }
  if (!target) throw new Error('no page target');
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
  let seq = 0;
  const pending = new Map();
  const consoleErrors = [];
  ws.onmessage = (m) => {
    const d = JSON.parse(m.data);
    if (d.id && pending.has(d.id)) { pending.get(d.id)(d); pending.delete(d.id); return; }
    if (d.method === 'Runtime.exceptionThrown') consoleErrors.push(`exception: ${d.params.exceptionDetails.exception?.description ?? d.params.exceptionDetails.text}`);
    if (d.method === 'Runtime.consoleAPICalled' && d.params.type === 'error') consoleErrors.push(`console.error: ${d.params.args.map((a) => a.value ?? a.description).join(' ')}`);
    if (d.method === 'Log.entryAdded' && d.params.entry.level === 'error') consoleErrors.push(`log: ${d.params.entry.text}`);
  };
  const send = (method, params = {}) => new Promise((res) => { const id = ++seq; pending.set(id, res); ws.send(JSON.stringify({ id, method, params })); });
  const ev = async (expression) => {
    const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (r.result?.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails).slice(0, 400));
    return r.result?.result?.value;
  };
  await send('Runtime.enable'); await send('Log.enable'); await send('Page.enable');
  await send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });
  for (let i = 0; i < 40; i++) {
    if (await ev(`!!document.querySelector('button[aria-label="Direct messages"]')`).catch(() => false)) break;
    await sleep(250);
  }

  const KEYS = { Enter: { code: 'Enter', vk: 13, text: '\r' }, Escape: { code: 'Escape', vk: 27 }, ArrowDown: { code: 'ArrowDown', vk: 40 }, ArrowUp: { code: 'ArrowUp', vk: 38 }, Tab: { code: 'Tab', vk: 9 } };
  async function press(key, modifiers = 0) {
    const k = KEYS[key] ?? { code: `Key${key.toUpperCase()}`, vk: key.toUpperCase().charCodeAt(0) };
    const base = { key, code: k.code, windowsVirtualKeyCode: k.vk, nativeVirtualKeyCode: k.vk, modifiers };
    await send('Input.dispatchKeyEvent', { type: 'rawKeyDown', ...base, text: k.text });
    if (k.text) await send('Input.dispatchKeyEvent', { type: 'char', ...base, text: k.text });
    await send('Input.dispatchKeyEvent', { type: 'keyUp', ...base });
    await sleep(200);
  }
  const typeText = async (text) => { await send('Input.insertText', { text }); await sleep(200); };
  async function center(selector, index = 0) {
    const r = await ev(`(() => { const n = document.querySelectorAll(${JSON.stringify(selector)})[${index}]; if (!n) return null; const r = n.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; })()`);
    if (!r) throw new Error(`no element ${selector}[${index}]`);
    return r;
  }
  async function hover(selector, index = 0) {
    const { x, y } = await center(selector, index);
    await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y });
    await sleep(150);
  }
  async function click(selector, index = 0) {
    const { x, y } = await center(selector, index);
    await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y });
    await send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: 'left', clickCount: 1 });
    await send('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: 'left', clickCount: 1 });
    await sleep(300);
  }
  async function moveAway() { await send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 1, y: 1 }); await sleep(100); }
  async function screenshot(path) {
    const r = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
    writeFileSync(path, Buffer.from(r.result.data, 'base64'));
  }
  async function resize(w, h) {
    await send('Emulation.setDeviceMetricsOverride', { width: w, height: h, deviceScaleFactor: 1, mobile: false });
    await sleep(300);
  }
  // The next launch reuses the debugging port, so wait for this browser to release it.
  const close = async () => { ws.close(); browser.kill(); await sleep(1500); };
  return { send, ev, press, typeText, hover, click, moveAway, screenshot, resize, close, consoleErrors, sleep };
}
