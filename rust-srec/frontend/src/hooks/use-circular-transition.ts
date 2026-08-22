import { useCallback, useRef } from 'react';
import { useTheme } from '@/components/providers/theme-provider';
import { getSystemTheme, type Mode } from '@/lib/theme-config';
import { flushSync } from 'react-dom';

interface CircularTransitionHook {
  setModeWithReveal: (next: Mode, coords: { x: number; y: number }) => void;
}

export function useCircularTransition(): CircularTransitionHook {
  const { resolvedMode, setMode } = useTheme();
  const isTransitioningRef = useRef(false);

  const startTransition = useCallback(
    (coords: { x: number; y: number }, callback: () => void) => {
      if (isTransitioningRef.current) return;
      if (typeof document === 'undefined') return;
      if (typeof window === 'undefined') return;

      const prefersReducedMotion = window.matchMedia(
        '(prefers-reduced-motion: reduce)',
      ).matches;

      // No View Transitions support (or reduced motion): just flip the theme.
      if (!('startViewTransition' in document) || prefersReducedMotion) {
        callback();
        return;
      }

      isTransitioningRef.current = true;

      // Reveal geometry consumed by the @keyframes in styles.css. Set on
      // <html> before startViewTransition so the pseudo-element animations
      // (which begin on the very first transition frame) already have their
      // clip-path origin/radius — avoids the flash where the new snapshot
      // paints unclipped before a JS-attached animation starts.
      const endRadius = Math.hypot(
        Math.max(coords.x, window.innerWidth - coords.x),
        Math.max(coords.y, window.innerHeight - coords.y),
      );
      const rootStyle = document.documentElement.style;
      rootStyle.setProperty('--reveal-x', `${coords.x}px`);
      rootStyle.setProperty('--reveal-y', `${coords.y}px`);
      rootStyle.setProperty('--reveal-r', `${endRadius}px`);

      const viewTransition = (
        document as Document & {
          startViewTransition: (callback: () => void) => {
            finished: Promise<void>;
          };
        }
      ).startViewTransition(() => {
        flushSync(() => {
          callback();
        });
      });

      // finished settles on every path: it fulfills even for skipped
      // transitions and rejects only if the update callback threw. Swallow
      // the rejection — main.desktop.tsx routes unhandledrejection to
      // renderFatal, which would replace the app with the boot-error screen
      // over a failed reveal — and release the lock in all cases.
      void viewTransition.finished
        .catch(() => {})
        .finally(() => {
          isTransitioningRef.current = false;
        });
    },
    [],
  );

  const setModeWithReveal = useCallback(
    (next: Mode, coords: { x: number; y: number }) => {
      const nextResolved = next === 'system' ? getSystemTheme() : next;
      if (nextResolved === resolvedMode) {
        // Persist the preference change (e.g. dark -> system while the OS is
        // dark); nothing changes visually, so skip the 400ms reveal.
        setMode(next);
        return;
      }
      startTransition(coords, () => setMode(next));
    },
    [resolvedMode, setMode, startTransition],
  );

  return {
    setModeWithReveal,
  };
}
