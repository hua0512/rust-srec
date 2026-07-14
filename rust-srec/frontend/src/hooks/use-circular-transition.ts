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

      isTransitioningRef.current = true;

      // Set CSS variables for the circular reveal animation - exactly like tweakcn
      const x = (coords.x / window.innerWidth) * 100;
      const y = (coords.y / window.innerHeight) * 100;

      // Set the CSS variables on document element
      document.documentElement.style.setProperty('--x', `${x}%`);
      document.documentElement.style.setProperty('--y', `${y}%`);

      const prefersReducedMotion = window.matchMedia(
        '(prefers-reduced-motion: reduce)',
      ).matches;

      // Check if View Transitions API is supported
      if ('startViewTransition' in document && !prefersReducedMotion) {
        const viewTransition = (
          document as Document & {
            startViewTransition: (callback: () => void) => {
              ready: Promise<void>;
              finished: Promise<void>;
            };
          }
        ).startViewTransition(() => {
          flushSync(() => {
            callback();
          });
        });

        const endRadius = Math.hypot(
          Math.max(coords.x, window.innerWidth - coords.x),
          Math.max(coords.y, window.innerHeight - coords.y),
        );

        void viewTransition.ready
          .then(() => {
            const clipPath = [
              `circle(0px at ${coords.x}px ${coords.y}px)`,
              `circle(${endRadius}px at ${coords.x}px ${coords.y}px)`,
            ];

            const targetIsDark =
              document.documentElement.classList.contains('dark');

            document.documentElement.animate(
              {
                clipPath: targetIsDark ? clipPath : [...clipPath].reverse(),
              },
              {
                duration: 400,
                easing: 'ease-in',
                pseudoElement: targetIsDark
                  ? '::view-transition-new(root)'
                  : '::view-transition-old(root)',
                fill: 'forwards',
              },
            );
          })
          .catch(() => {
            // ready rejects when the reveal is skipped (document hidden,
            // superseded by another view transition). The mode change already
            // committed inside the update callback — only the animation is
            // lost, so there is nothing to recover.
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
      } else {
        // Fallback for browsers without View Transitions API
        callback();
        setTimeout(() => {
          isTransitioningRef.current = false;
        }, 400);
      }
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
