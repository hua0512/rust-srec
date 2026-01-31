import './styles.css';

import '@fontsource/inter/400.css';
import '@fontsource/inter/500.css';
import '@fontsource/inter/600.css';
import '@fontsource/inter/700.css';

import React from 'react';
import { createRoot } from 'react-dom/client';
import { RouterProvider } from '@tanstack/react-router';

import { isTauriRuntime } from '@/utils/tauri';

import { getRouter } from './router.desktop';
import {
  createI18nInstance,
  defaultLocale,
  dynamicActivate,
  getPreferredLocale,
  isLocaleValid,
  localeStorageKey,
  type Locale,
} from './integrations/lingui/i18n';

const rootEl = document.getElementById('root')!;

function escapeHtml(input: string): string {
  return input
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

let frontendReadyNotified = false;

function getBootError(): string | null {
  const raw = (globalThis as any).__RUST_SREC_BOOT_ERROR__;
  return typeof raw === 'string' && raw.trim().length > 0 ? raw : null;
}

async function notifyFrontendReady(): Promise<void> {
  if (frontendReadyNotified) return;
  frontendReadyNotified = true;

  if (!isTauriRuntime()) return;

  try {
    const { emit } = await import('@tauri-apps/api/event');
    await emit('rust-srec://frontend-ready');
  } catch {
    // best-effort
  }
}

function renderFatal(error: unknown) {
  const message =
    error instanceof Error
      ? `${error.name}: ${error.message}\n\n${error.stack ?? ''}`
      : String(error);

  rootEl.innerHTML = `
    <div style="min-height:100vh;display:flex;align-items:center;justify-content:center;padding:24px;background:#0b1220;color:#e5e7eb;">
      <div style="max-width:920px;width:100%;">
        <div style="font:600 18px/1.2 system-ui, -apple-system, Segoe UI, Roboto, sans-serif;margin-bottom:12px;">Rust-Srec (Desktop) failed to start</div>
        <div style="font:400 13px/1.5 ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, 'Liberation Mono', 'Courier New', monospace;white-space:pre-wrap;background:#0f172a;border:1px solid #24324a;border-radius:12px;padding:14px;">${escapeHtml(message)}</div>
        <div style="margin-top:12px;font:400 12px/1.4 system-ui, -apple-system, Segoe UI, Roboto, sans-serif;color:#a7b0bf;">Open DevTools to see console details.</div>
      </div>
    </div>
  `;

  void notifyFrontendReady();
}

window.addEventListener('error', (e) => {
  renderFatal((e as ErrorEvent).error ?? (e as ErrorEvent).message);
});

window.addEventListener('unhandledrejection', (e) => {
  renderFatal((e as PromiseRejectionEvent).reason);
});

async function resolveInitialLocale(): Promise<Locale> {
  try {
    const stored = window.localStorage.getItem(localeStorageKey);
    if (stored && isLocaleValid(stored)) return stored;
  } catch {
    // ignore localStorage errors
  }

  const preferred = getPreferredLocale(navigator.language);
  return preferred ?? defaultLocale;
}

async function bootstrap() {
  const bootError = getBootError();
  if (bootError) {
    renderFatal(bootError);
    return;
  }

  const i18n = createI18nInstance();
  const locale = await resolveInitialLocale();

  if (import.meta.env.DEV) {
    console.info('[desktop] activating locale', locale);
  }

  // Activate immediately so Lingui's provider does not render `null`.
  // We load catalogs asynchronously after the initial render.
  i18n.load(locale, {});
  i18n.activate(locale);

  if (import.meta.env.DEV) {
    console.info('[desktop] active locale', i18n.locale);
  }

  const router = getRouter(i18n);
  createRoot(rootEl).render(
    <React.StrictMode>
      <RouterProvider router={router} />
    </React.StrictMode>,
  );

  // Tell the native host we're ready to show the main window.
  await notifyFrontendReady();

  // Load the catalog after the shell is visible.
  dynamicActivate(i18n, locale).catch(() => undefined);
}

bootstrap().catch((e) => {
  renderFatal(e);
});
