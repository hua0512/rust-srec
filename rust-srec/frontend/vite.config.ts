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
        manualChunks: (id) => {
          if (id.includes('node_modules')) {
            if (id.includes('artplayer')) return 'vendor-player-art';
            if (id.includes('hls.js')) return 'vendor-player-hls';
            if (id.includes('mpegts.js')) return 'vendor-player-mpegts';

            // UI Libraries
            if (
              id.includes('@radix-ui') ||
              id.includes('lucide-react') ||
              id.includes('motion') ||
              id.includes('sonner')
            ) {
              return 'vendor-ui';
            }

            // Utils
            if (
              id.includes('date-fns') ||
              id.includes('zod') ||
              id.includes('react-hook-form') ||
              id.includes('ky')
            ) {
              return 'vendor-utils';
            }

            // We avoid bundling react/tanstack explicitly here to let Vite handle common chunking
          }
        },
      },
    },
  },
}));
