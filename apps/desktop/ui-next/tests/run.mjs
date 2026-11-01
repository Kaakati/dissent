// Runs the UI driver (driver.mjs) against a production build of the app with the mock: `vite build` into a scratch folder,
// then `vite preview` serves that folder. No dev server, so no hot reloads can change the page mid-run. The exit code is
// the driver's: non-zero when a check fails, or when the build, the server or the browser cannot start.
import { spawn } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { freePort } from './cdp.mjs';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..');
// A free port of this run's own, unless UI_PORT names one.
const port = process.env.UI_PORT ?? String(await freePort());
// Each run gets its own scratch folder, so a browser left by an earlier run cannot hold a profile this run needs.
const scratch = process.env.SCRATCH ?? join(tmpdir(), 'dissent-ui-driver', `run-${Date.now()}`);
const outDir = join(scratch, 'ui-build');
const vite = join(root, 'node_modules/vite/bin/vite.js');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

mkdirSync(scratch, { recursive: true });

// Runs one node script and resolves with its exit code.
const run = (args, options = {}) => new Promise((resolve) => {
  const child = spawn(process.execPath, args, { cwd: root, stdio: 'inherit', ...options });
  child.on('exit', (code) => resolve(code ?? 1));
});

// Something already answering on the port would be used instead of the preview, so the run refuses to start.
async function assertPortFree() {
  try {
    await fetch(`http://127.0.0.1:${port}/`);
  } catch {
    return;
  }
  throw new Error(`port ${port} is already in use: stop the server on it, or set UI_PORT`);
}

// Throwaway fixture art, so the build ships known built-ins whatever the real art folder holds: a looping GIF of a ring
// with a clear middle, written with an LZW stream that clears before its codes outgrow 8 bits.
function fixtureGif(side, frames) {
  const palette = [0, 0, 0, 255, 60, 90, 90, 220, 180, 60, 255, 255];
  const packed = (pixels) => {
    const out = [7];
    for (let i = 0; i < pixels.length; i += 100) {
      const run = [128, ...pixels.slice(i, i + 100)];
      if (i + 100 >= pixels.length) run.push(129);
      for (let at = 0; at < run.length; at += 255) {
        const block = run.slice(at, at + 255);
        out.push(block.length, ...block);
      }
    }
    out.push(0);
    return out;
  };
  const le = (n) => [n & 255, n >> 8];
  const bytes = [...Buffer.from('GIF89a'), ...le(side), ...le(side), 0xf6, 0, 0, ...palette, ...new Array(128 * 3 - palette.length).fill(0)];
  bytes.push(0x21, 0xff, 11, ...Buffer.from('NETSCAPE2.0'), 3, 1, 0, 0, 0);
  for (let f = 0; f < frames; f++) {
    const pixels = [];
    for (let y = 0; y < side; y++) {
      for (let x = 0; x < side; x++) {
        const d = Math.hypot(x - side / 2 + 0.5, y - side / 2 + 0.5) / (side / 2);
        pixels.push(d > 0.84 && d < 1 ? 1 + ((f + Math.floor(Math.atan2(y - side / 2, x - side / 2) * 4 + 8)) % 3) : 0);
      }
    }
    bytes.push(0x21, 0xf9, 4, 0x09, 5, 0, 0, 0, 0x2c, 0, 0, 0, 0, ...le(side), ...le(side), 0, ...packed(pixels), );
  }
  bytes.push(0x3b);
  return Buffer.from(bytes);
}

async function build() {
  const art = join(scratch, 'decorations');
  mkdirSync(art, { recursive: true });
  writeFileSync(join(art, 'ring.gif'), fixtureGif(64, 3));
  writeFileSync(join(art, 'wide.gif'), fixtureGif(48, 2));
  const list = join(art, 'list.json');
  writeFileSync(list, JSON.stringify([
    { id: 'fixture_ring', name: 'Fixture ring', file: 'ring.gif' },
    { id: 'fixture_small', name: 'Fixture small', file: 'wide.gif' },
    { id: 'fixture_absent', name: 'Fixture absent', file: 'not-there.gif' },
  ]));
  const env = { ...process.env, DISSENT_DECORATIONS_DIR: art, DISSENT_DECORATIONS_LIST: list };
  const code = await run([vite, 'build', '--outDir', outDir, '--emptyOutDir'], { env });
  if (code !== 0) throw new Error(`vite build failed (exit ${code})`);
}

// Starts the preview server. Resolves to a function that stops it.
async function serve() {
  const preview = spawn(process.execPath, [vite, 'preview', '--outDir', outDir, '--port', port, '--strictPort', '--host', '127.0.0.1'], {
    cwd: root,
    stdio: 'ignore',
  });
  let exited = false;
  preview.on('exit', () => { exited = true; });
  for (let i = 0; i < 120; i++) {
    if (exited) throw new Error(`vite preview exited before it served on port ${port} (is the port in use?)`);
    try {
      const html = await (await fetch(`http://127.0.0.1:${port}/`)).text();
      // A dev server's page loads its modules from /src; the build loads them from /assets.
      if (!html.includes('/assets/') || html.includes('/src/main')) throw new Error('the page on the port is not the build');
      return () => preview.kill();
    } catch (err) {
      if (err.message === 'the page on the port is not the build') {
        preview.kill();
        throw err;
      }
      // not listening yet
    }
    await sleep(500);
  }
  preview.kill();
  throw new Error(`vite preview did not serve on port ${port} within 60 s`);
}

let code = 1;
let stop = () => {};
try {
  await assertPortFree();
  await build();
  stop = await serve();
  code = await run([join(here, 'driver.mjs')], { env: { ...process.env, UI_PORT: port, SCRATCH: scratch } });
} catch (err) {
  console.error(err.message);
} finally {
  stop();
}
process.exit(code);
