import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { defineConfig } from 'vite';
import viteReact from '@vitejs/plugin-react';
import viteTsConfigPaths from 'vite-tsconfig-paths';
import tailwindcss from '@tailwindcss/vite';
import { devtools } from '@tanstack/devtools-vite';
import { tanstackRouter } from '@tanstack/router-plugin/vite';
import { lingui } from '@lingui/vite-plugin';
import oxlintPlugin from 'vite-plugin-oxlint';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  plugins: [
    lingui(),
    devtools(),
    // this is the plugin that enables path aliases
    viteTsConfigPaths({
      projects: ['./tsconfig.json'],
    }),
    tailwindcss(),
    tanstackRouter({
      target: 'react',
      autoCodeSplitting: true,
    }),
    viteReact({
      babel: {
        plugins: ['macros'],
      },
    }),
    // Limit oxlint to source folders (avoid linting build outputs).
    oxlintPlugin({ path: 'src' }),
  ],
  resolve: {
    alias: [
      // Desktop build does not run a TanStack Start server.
      // Replace server functions with a direct in-browser implementation.
      {
        find: /^@\/server\/createServerFn$/,
        replacement: path.resolve(
          __dirname,
          'src/server/createServerFn.desktop.ts',
        ),
      },
      // Hard-stop any accidental TanStack Start runtime imports.
      {
        find: /^@tanstack\/react-start$/,
        replacement: path.resolve(__dirname, 'src/desktop/start-stub.ts'),
      },
      {
        find: /^@tanstack\/react-start\/server$/,
        replacement: path.resolve(__dirname, 'src/desktop/start-stub.ts'),
      },
    ],
  },
  server: {
    host: '127.0.0.1',
    port: 15275,
    strictPort: true,
    proxy: {
      // Dev-only: keep matching the backend default when running the web stack.
      '/api': {
        target: 'http://127.0.0.1:12555',
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: 'dist-desktop',
    emptyOutDir: true,
    chunkSizeWarningLimit: 600,
    rollupOptions: {
      input: {
        desktop: path.resolve(__dirname, 'index.desktop.html'),
        loading: path.resolve(__dirname, 'desktop-loading.html'),
      },
      output: {
        advancedChunks: {
          groups: [
            { name: 'vendor-player-art', test: /node_modules[\\/]artplayer/ },
            { name: 'vendor-player-hls', test: /node_modules[\\/]hls\\.js/ },
            {
              name: 'vendor-player-mpegts',
              test: /node_modules[\\/]mpegts\\.js/,
            },
          ],
        },
      },
    },
  },
});
