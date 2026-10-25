// Runs the UI driver (driver.mjs) against a production build of the app with the mock: `vite build` into a scratch folder,
// then `vite preview` serves that folder. No dev server, so no hot reloads can change the page mid-run. The exit code is
// the driver's: non-zero when a check fails, or when the build, the server or the browser cannot start.
import { spawn } from 'node:child_process';
import { mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, '..');
const port = process.env.UI_PORT ?? '1420';
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

async function build() {
  const code = await run([vite, 'build', '--outDir', outDir, '--emptyOutDir']);
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
