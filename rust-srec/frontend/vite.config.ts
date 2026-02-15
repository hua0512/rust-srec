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
    lingui(),
    devtools(),
    nitro(),
    // this is the plugin that enables path aliases
    viteTsConfigPaths({
      projects: ['./tsconfig.json'],
    }),
    tailwindcss(),
    tanstackStart({}),
    viteReact({
      babel: {
        plugins: ['macros'],
      },
    }),
    // Limit oxlint to source folders (avoid linting build outputs).
    oxlintPlugin({ path: 'src' }),
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
            { name: 'vendor-player-art', test: /node_modules[\\/]artplayer/ },
            { name: 'vendor-player-hls', test: /node_modules[\\/]hls\\.js/ },
            {
              name: 'vendor-player-mpegts',
              test: /node_modules[\\/]mpegts\\.js/,
            },
            { name: 'vendor-radix', test: /node_modules[\\/]@radix-ui/ },
            { name: 'vendor-motion', test: /node_modules[\\/]motion/ },
            { name: 'vendor-xyflow', test: /node_modules[\\/]@xyflow/ },
            { name: 'vendor-date-fns', test: /node_modules[\\/]date-fns/ },
            {
              name: 'vendor-react-hook-form',
              test: /node_modules[\\/]react-hook-form/,
            },
            { name: 'vendor-protobuf', test: /node_modules[\\/]@bufbuild/ },
            {
              name: 'vendor-tanstack',
              test: /node_modules[\\/]@tanstack[\\/]react-query/,
            },
            { name: 'vendor-zod', test: /node_modules[\\/]zod/ },
          ],
        },
      },
    },
  },
}));
