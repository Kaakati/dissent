import { existsSync, readFileSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig, normalizePath, type Plugin } from 'vite'

const here = path.dirname(fileURLToPath(import.meta.url))

// The built-in avatar decorations. decorations.json is the one list of what ships, and only art with confirmed rights
// goes on it: an entry names an id (on the wire for good, so never reuse one), a label and a file in assets/decorations.
// An entry whose file is absent is skipped, so the app offers exactly what the build holds.
const decorationsDir = process.env.DISSENT_DECORATIONS_DIR ?? path.resolve(here, '../../../assets/decorations')
const decorationsList = process.env.DISSENT_DECORATIONS_LIST ?? path.resolve(here, 'decorations.json')

function decorations(): Plugin {
  const virtual = 'virtual:decorations'
  return {
    name: 'dissent-decorations',
    resolveId: (id) => (id === virtual ? `\0${virtual}` : undefined),
    load(id) {
      if (id !== `\0${virtual}`) return undefined
      this.addWatchFile(decorationsList)
      const list: { id: string; name: string; file: string }[] = JSON.parse(readFileSync(decorationsList, 'utf8'))
      const shipped = list.filter((d) => {
        const here = existsSync(path.join(decorationsDir, d.file))
        if (!here) this.warn(`decoration ${d.id}: ${d.file} is not in ${decorationsDir}; left out of this build`)
        return here
      })
      const imports = shipped.map((d, i) => `import a${i} from ${JSON.stringify(`${normalizePath(path.join(decorationsDir, d.file))}?url`)};`)
      const items = shipped.map((d, i) => `{ id: ${JSON.stringify(d.id)}, name: ${JSON.stringify(d.name)}, url: a${i} }`)
      return `${imports.join('\n')}\nexport default [${items.join(', ')}];`
    },
  }
}

export default defineConfig({
  plugins: [react(), tailwindcss(), decorations()],
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
    // DISCLAIMER.md at the repository root is bundled too (lib/disclaimer.ts).
    fs: { allow: [path.resolve(here, '..'), path.resolve(here, '../../../DISCLAIMER.md'), decorationsDir] },
  },
})
