import './styles.css';

import React from 'react';
import { createRoot } from 'react-dom/client';
import { RouterProvider } from '@tanstack/react-router';

import { isTauriRuntime } from '@/utils/tauri';

import {
  createFrontendFailure,
  FatalScreen,
  parseBootFailure,
  type BootFailurePayload,
} from './desktop/fatal-screen';
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
const root = createRoot(rootEl);

let frontendReadyNotified = false;

function getBootError(): ReturnType<typeof parseBootFailure> {
  const raw = (
    globalThis as typeof globalThis & {
      __RUST_SREC_BOOT_ERROR__?: BootFailurePayload | string | null;
    }
  ).__RUST_SREC_BOOT_ERROR__;
  return parseBootFailure(raw);
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
  root.render(<FatalScreen failure={createFrontendFailure(error)} />);

  void notifyFrontendReady();
}

function renderBootFailure(
  failure: NonNullable<ReturnType<typeof getBootError>>,
) {
  root.render(<FatalScreen failure={failure} />);
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
    renderBootFailure(bootError);
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

  // Resolved by NotifyFirstCommit's effect after React's first commit.
  // Effects fire even while the window is hidden, unlike animation frames.
  let signalFirstCommit!: () => void;
  const firstCommit = new Promise<void>((resolve) => {
    signalFirstCommit = resolve;
  });
  function NotifyFirstCommit() {
    React.useEffect(() => {
      signalFirstCommit();
    }, []);
    return null;
  }

  root.render(
    <React.StrictMode>
      <NotifyFirstCommit />
      <RouterProvider router={router} />
    </React.StrictMode>,
  );

  await firstCommit;

  // Best-effort wait for a presented frame on top of the commit. The main
  // window is created hidden and only shown by lib.rs on
  // 'rust-srec://frontend-ready', and hidden webviews may suspend
  // requestAnimationFrame entirely — so bound the wait instead of trusting it
  // (an unbounded wait would ride lib.rs's 6s show-anyway fallback on every
  // launch). The pre-paint script in index.desktop.html already guarantees
  // the revealed frame is themed either way.
  await Promise.race([
    new Promise<void>((resolve) =>
      requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
    ),
    new Promise<void>((resolve) => setTimeout(resolve, 120)),
  ]);

  await notifyFrontendReady();

  // Load the catalog after the shell is visible.
  dynamicActivate(i18n, locale).catch(() => undefined);
}

bootstrap().catch((e) => {
  renderFatal(e);
});
