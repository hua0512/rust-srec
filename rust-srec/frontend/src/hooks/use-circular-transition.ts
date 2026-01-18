import { useCallback, useRef } from 'react';
import { useTheme } from '@/components/providers/theme-provider';
import { flushSync } from 'react-dom';

interface CircularTransitionHook {
  startTransition: (
    coords: { x: number; y: number },
    callback: () => void,
  ) => void;
  toggleTheme: (event: React.MouseEvent) => void;
  isTransitioning: () => boolean;
}

export function useCircularTransition(): CircularTransitionHook {
  const { theme, setTheme } = useTheme();
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

        viewTransition.ready
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
          .finally(() => {
            // Ensure we always release the lock even if animation fails.
            void viewTransition.finished.finally(() => {
              isTransitioningRef.current = false;
            });
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

  const toggleTheme = useCallback(
    (event: React.MouseEvent) => {
      // Get precise click coordinates - use clientX/clientY directly like tweakcn
      const coords = {
        x: event.clientX,
        y: event.clientY,
      };

      startTransition(coords, () => {
        setTheme(theme === 'dark' ? 'light' : 'dark');
      });
    },
    [theme, setTheme, startTransition],
  );

  const isTransitioning = useCallback(() => {
    return isTransitioningRef.current;
  }, []);

  return {
    startTransition,
    toggleTheme,
    isTransitioning,
  };
}
