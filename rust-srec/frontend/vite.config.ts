import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { defineConfig } from 'vite';
import { devtools } from '@tanstack/devtools-vite';
import { tanstackStart } from '@tanstack/react-start/plugin/vite';
import viteReact from '@vitejs/plugin-react';
import viteTsConfigPaths from 'vite-tsconfig-paths';
import tailwindcss from '@tailwindcss/vite';
import { nitro } from 'nitro/vite';

import { lingui } from '@lingui/vite-plugin';
import oxlintPlugin from 'vite-plugin-oxlint';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

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
  resolve: {
    alias: [
      // Ensure pbjs-generated `import * as $protobuf from "protobufjs/minimal";`
      // works in SSR (ESM) by routing it through an ESM shim.
      {
        // IMPORTANT: do NOT match `protobufjs/minimal.js` here.
        // Our shim imports `protobufjs/minimal.js` to get the actual CommonJS
        // implementation; if we alias `.js` too, we create a self-import cycle
        // (and hit `Cannot access 'pb' before initialization`).
        find: /^protobufjs\/minimal$/,
        replacement: path.resolve(
          __dirname,
          'src/api/proto/protobufjs-minimal.ts',
        ),
      },
    ],
  },
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
          ],
        },
      },
    },
  },
}));
