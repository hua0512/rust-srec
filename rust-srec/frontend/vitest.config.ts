import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';
import viteReact from '@vitejs/plugin-react';
import viteTsConfigPaths from 'vite-tsconfig-paths';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

export default defineConfig({
  plugins: [
    viteTsConfigPaths({
      projects: ['./tsconfig.json'],
    }),
    viteReact({
      babel: {
        plugins: ['macros'],
      },
    }),
  ],
  resolve: {
    alias: [
      {
        find: /^protobufjs\/minimal$/,
        replacement: path.resolve(
          __dirname,
          'src/api/proto/protobufjs-minimal.ts',
        ),
      },
    ],
  },
  test: {
    environment: 'jsdom',
    globals: true,
    include: ['**/*.{test,spec}.?(c|m)[jt]s?(x)'],
    passWithNoTests: true,
  },
});
