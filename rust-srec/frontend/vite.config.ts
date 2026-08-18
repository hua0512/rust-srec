import { defineConfig, type Plugin } from 'vite';
import { lingui, linguiTransformerBabelPreset } from '@lingui/vite-plugin';
import babel from '@rolldown/plugin-babel';
import { devtools } from '@tanstack/devtools-vite';
import { tanstackStart } from '@tanstack/react-start/plugin/vite';
import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { nitro } from 'nitro/vite';

import oxlintPlugin from 'vite-plugin-oxlint';

import { computeThemeCacheId } from './theme-cache-id.ts';

const IMMUTABLE_ASSET_CACHE_CONTROL = 'public, max-age=31536000, immutable';
const PUBLIC_LOGO_CACHE_CONTROL =
  'public, max-age=604800, stale-while-revalidate=86400';

function previewCacheHeaders(): Plugin {
  return {
    name: 'preview-cache-headers',
    enforce: 'pre',
    configurePreviewServer(server) {
      server.middlewares.use((req, res, next) => {
        if (req.url?.startsWith('/assets/')) {
          res.setHeader('Cache-Control', IMMUTABLE_ASSET_CACHE_CONTROL);
        } else if (
          req.url?.startsWith('/stream-rec.svg') ||
          req.url?.startsWith('/stream-rec-white.svg')
        ) {
          res.setHeader('Cache-Control', PUBLIC_LOGO_CACHE_CONTROL);
        }
        next();
      });
    },
  };
}

export default defineConfig(() => ({
  define: {
    __THEME_CACHE_ID__: JSON.stringify(computeThemeCacheId()),
  },
  plugins: [
    lingui(),
    devtools(),
    previewCacheHeaders(),
    nitro({
      routeRules: {
        '/assets/**': {
          headers: {
            'cache-control': IMMUTABLE_ASSET_CACHE_CONTROL,
          },
        },
        '/stream-rec.svg': {
          headers: {
            'cache-control': PUBLIC_LOGO_CACHE_CONTROL,
          },
        },
        '/stream-rec-white.svg': {
          headers: {
            'cache-control': PUBLIC_LOGO_CACHE_CONTROL,
          },
        },
      },
    }),
    tailwindcss(),
    tanstackStart({}),
    react(),
    babel({ presets: [linguiTransformerBabelPreset()] }),
    // Limit oxlint to source folders (avoid linting build outputs).
    oxlintPlugin({ path: 'src' }),
  ],
  resolve: {
    tsconfigPaths: true,
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
  },
}));
