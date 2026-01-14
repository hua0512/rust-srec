import { defineConfig } from 'vite';
import { devtools } from '@tanstack/devtools-vite';
import { tanstackStart } from '@tanstack/react-start/plugin/vite';
import viteReact from '@vitejs/plugin-react';
import viteTsConfigPaths from 'vite-tsconfig-paths';
import tailwindcss from '@tailwindcss/vite';
import { nitro } from 'nitro/vite';

import { lingui } from '@lingui/vite-plugin';
import oxlintPlugin from 'vite-plugin-oxlint';

export default defineConfig(() => ({
  plugins: [
    devtools(),
    nitro(),
    // this is the plugin that enables path aliases
    viteTsConfigPaths({
      projects: ['./tsconfig.json'],
    }),
    tailwindcss(),
    tanstackStart(),
    viteReact({
      babel: {
        plugins: ['macros'],
      },
    }),
    lingui(),
    oxlintPlugin(),
  ],
  server: {
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:12555',
        changeOrigin: true,
      },
    },
  },
  build: {
    chunkSizeWarningLimit: 600,
    rollupOptions: {
      output: {
        advancedChunks: {
          groups: [
            { name: 'vendor-player-art', test: /artplayer/ },
            { name: 'vendor-player-hls', test: /hls\.js/ },
            { name: 'vendor-player-mpegts', test: /mpegts\.js/ },
            { name: 'vendor-ui', test: /@radix-ui|lucide-react|motion|sonner/ },
            { name: 'vendor-utils', test: /date-fns|zod|react-hook-form|ky/ },
          ],
        },
      },
    },
  },
}));
