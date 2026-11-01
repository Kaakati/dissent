// The built-in avatar decorations this build ships. Made by the plugin in vite.config.ts from decorations.json.
declare module 'virtual:decorations' {
  const builtins: { id: string; name: string; url: string }[];
  export default builtins;
}
