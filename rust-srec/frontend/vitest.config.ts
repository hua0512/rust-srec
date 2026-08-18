import path from 'node:path';
import { defineConfig } from 'vitest/config';
import { linguiTransformerBabelPreset } from '@lingui/vite-plugin';
import babel from '@rolldown/plugin-babel';
import react from '@vitejs/plugin-react';

export default defineConfig({
  plugins: [react(), babel({ presets: [linguiTransformerBabelPreset()] })],
  resolve: {
    alias: {
      '@': path.resolve(import.meta.dirname, './src'),
    },
  },
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    include: ['**/*.{test,spec}.?(c|m)[jt]s?(x)'],
    passWithNoTests: true,
  },
});
