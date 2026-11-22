import { useEffect, useRef, useState } from 'react';
import { plural, t } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Trans } from '@lingui/react/macro';
import { Loader2 } from 'lucide-react';

import { isDesktopBuild } from '@/utils/desktop';
import { isTauriRuntime } from '@/utils/tauri';

const SHUTDOWN_EVENT = 'rust-srec://desktop-shutting-down';

interface DesktopShuttingDownPayload {
  activeRecordings: number;
}

export function DesktopShutdownListener() {
  const { i18n } = useLingui();
  const [shutdownState, setShutdownState] =
    useState<DesktopShuttingDownPayload | null>(null);
  const overlayRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!isDesktopBuild() || !isTauriRuntime()) return;

    let alive = true;
    let unlisten: (() => void) | null = null;

    void (async () => {
      try {
        const { listen } = await import('@tauri-apps/api/event');
        const nextUnlisten = await listen<DesktopShuttingDownPayload>(
          SHUTDOWN_EVENT,
          (event) => {
            if (!alive) return;
            const rawCount = event.payload?.activeRecordings;
            const activeRecordings =
              typeof rawCount === 'number' &&
              Number.isFinite(rawCount) &&
              rawCount >= 0
                ? Math.trunc(rawCount)
                : 0;
            setShutdownState({ activeRecordings });
          },
        );

        if (!alive) {
          nextUnlisten();
          return;
        }

        unlisten = nextUnlisten;
      } catch (error) {
        console.error(
          '[desktop] failed to subscribe to shutdown event:',
          error,
        );
      }
    })();

    return () => {
      alive = false;
      if (unlisten) unlisten();
    };
  }, []);

  useEffect(() => {
    if (!shutdownState) return;

    const overlay = overlayRef.current;
    overlay?.focus();

    // Mark background sibling elements inert so keyboard users cannot activate
    // controls behind the modal shutdown screen while recordings are draining.
    const modifiedElements: Array<{ el: HTMLElement; wasInert: boolean }> = [];
    const parent = overlay?.parentElement;
    if (parent) {
      for (const child of Array.from(parent.children)) {
        if (child !== overlay && child instanceof HTMLElement) {
          modifiedElements.push({ el: child, wasInert: child.inert });
          child.inert = true;
        }
      }
    }

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Tab') {
        event.preventDefault();
        overlay?.focus();
      }
    };
    window.addEventListener('keydown', handleKeyDown, true);

    return () => {
      window.removeEventListener('keydown', handleKeyDown, true);
      for (const { el, wasInert } of modifiedElements) {
        el.inert = wasInert;
      }
    };
  }, [shutdownState]);

  if (!shutdownState) return null;

  return (
    <div
      ref={overlayRef}
      role="alertdialog"
      aria-modal="true"
      aria-live="assertive"
      aria-busy="true"
      aria-labelledby="desktop-shutdown-title"
      aria-describedby="desktop-shutdown-description"
      tabIndex={-1}
      className="fixed inset-0 z-[200] flex items-center justify-center bg-background/80 p-6 backdrop-blur-sm focus:outline-none"
    >
      <div className="flex w-full max-w-md flex-col items-center gap-4 rounded-xl border border-border bg-card p-6 text-center text-card-foreground shadow-2xl">
        <div className="flex size-12 items-center justify-center rounded-full bg-primary/10 text-primary">
          <Loader2 className="size-6 animate-spin" aria-hidden="true" />
        </div>

        <div className="space-y-2">
          <h2
            id="desktop-shutdown-title"
            className="text-lg font-semibold tracking-tight text-foreground"
          >
            <Trans>Finalizing recordings before exit…</Trans>
          </h2>
          <p
            id="desktop-shutdown-description"
            className="text-sm leading-relaxed text-muted-foreground"
          >
            {t(i18n)`Stopping and saving ${plural(
              shutdownState.activeRecordings,
              {
                one: '# active recording',
                other: '# active recordings',
              },
            )} cleanly so files are not corrupted.`}
          </p>
        </div>
      </div>
    </div>
  );
}
