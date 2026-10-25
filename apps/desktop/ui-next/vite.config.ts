import path from 'node:path'
import { fileURLToPath } from 'node:url'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

const here = path.dirname(fileURLToPath(import.meta.url))

export default defineConfig({
  plugins: [react(), tailwindcss()],
  // Relative asset URLs: the Tauri build loads the page from its own protocol, as the current UI does.
  base: './',
  resolve: {
    alias: { '@': path.resolve(here, 'src') },
  },
  server: {
    // The dev port tauri.conf.json would use. scripts/dev-next.sh points the app here without editing that file.
    port: 1420,
    strictPort: true,
    host: '127.0.0.1',
    // The browser mock is src/mock/mock.js. The allow list keeps Vite's file access as it was when the mock lived in ../ui.
    fs: { allow: [path.resolve(here, '..')] },
  },
})
