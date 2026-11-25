import { defineConfig, type Plugin } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';

/**
 * Keeps only modern `.woff2` KaTeX fonts (supported by WKWebView, WebView2, and WebKitGTK 4.1),
 * eliminating ~850 KB of legacy `.ttf`/`.woff` duplicates from the native installer bundle.
 */
function stripLegacyKatexFonts(): Plugin {
  return {
    name: 'strip-legacy-katex-fonts',
    enforce: 'pre',
    transform(code, id) {
      const cleanId = id.split('?')[0].replace(/\\/g, '/');
      if (cleanId.includes('node_modules/katex/') && cleanId.endsWith('.css')) {
        return {
          code: code
            .replace(/,\s*url\([^)]+\.woff\)\s*format\(["']woff["']\)/g, '')
            .replace(/,\s*url\([^)]+\.ttf\)\s*format\(["']truetype["']\)/g, ''),
          map: null,
        };
      }
      return null;
    },
  };
}

export default defineConfig({
  plugins: [stripLegacyKatexFonts(), svelte()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ['**/src-tauri/**', '**/crates/**'],
    },
  },
  build: {
    outDir: 'dist',
    target: 'esnext',
    rollupOptions: {
      output: {
        manualChunks(id) {
          const norm = id.replace(/\\/g, '/');
          if (norm.includes('node_modules/pdfjs-dist')) {
            return 'pdfjs';
          }
          if (norm.includes('node_modules/katex')) {
            return 'katex';
          }
        },
      },
    },
  },
});
