import { readFileSync } from 'node:fs';
import { defineConfig } from 'vite';

export default defineConfig({
  clearScreen: false,
  server: {
    // Not Jay-C's 5173, Jay-A's 5175 nor Jay-H's 5176, so the four apps can run in dev at once.
    port: 5177,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_'],
  // The app's version (Settings › About), bumped with each release.
  define: { __APP_VERSION__: JSON.stringify(JSON.parse(readFileSync(new URL('./package.json', import.meta.url), 'utf8')).version) },
  build: {
    // WebView2 (Chromium) on Windows.
    target: ['es2022', 'chrome110'],
    minify: process.env.TAURI_ENV_DEBUG ? false : undefined,
    sourcemap: Boolean(process.env.TAURI_ENV_DEBUG),
  },
});
