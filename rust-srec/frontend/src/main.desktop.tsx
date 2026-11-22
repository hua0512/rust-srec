import './styles.css';

import React from 'react';
import { createRoot } from 'react-dom/client';
import { RouterProvider } from '@tanstack/react-router';
import { setupI18n, type I18n } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import { toast } from 'sonner';

import { isTauriRuntime } from '@/utils/tauri';

import { DesktopErrorBoundary } from './desktop/desktop-error-boundary';
import {
  createFrontendFailure,
  FatalScreen,
  parseBootFailure,
  type BootFailurePayload,
} from './desktop/fatal-screen';
import {
  createDesktopErrorController,
  FALLBACK_ERROR_MESSAGE,
} from './desktop/runtime-error-handler';
import { getRouter } from './router.desktop';
import {
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

let activeI18n: I18n | null = null;

const errorController = createDesktopErrorController({
  onBootFatal: (error) => {
    renderFatal(error);
  },
  onRuntimeError: (message, error) => {
    console.error('[desktop] unhandled runtime error:', error);
    const localized =
      message === FALLBACK_ERROR_MESSAGE && activeI18n
        ? activeI18n._(msg`An unexpected error occurred`)
        : message;
    toast.error(localized);
  },
});

window.addEventListener('error', (e) => {
  errorController.handleError(e);
});

window.addEventListener('unhandledrejection', (e) => {
  errorController.handleError((e as PromiseRejectionEvent).reason);
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
    // Keep the native startup failure on screen even if a secondary error fires
    // while rendering the recovery UI.
    errorController.markMounted();
    renderBootFailure(bootError);
    return;
  }

  const i18n = setupI18n();
  activeI18n = i18n;
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
      <DesktopErrorBoundary>
        <RouterProvider router={router} />
      </DesktopErrorBoundary>
    </React.StrictMode>,
  );

  await firstCommit;
  errorController.markMounted();

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
  errorController.handleError(e);
});
