import { useTheme } from 'next-themes';
import { Moon, Sun } from 'lucide-react';
import { Trans } from '@lingui/react/macro';

import { Button } from '@/components/ui/button';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
  TooltipProvider,
} from '@/components/ui/tooltip';
import { flushSync } from 'react-dom';

export function ModeToggle() {
  const { setTheme, theme } = useTheme();

  const switchTheme = (e: React.MouseEvent) => {
    const newTheme = theme === 'dark' ? 'light' : 'dark';

    // We need the coordinates *before* the transition starts.
    const x = e.clientX;
    const y = e.clientY;

    if (
      !document.startViewTransition ||
      window.matchMedia('(prefers-reduced-motion: reduce)').matches
    ) {
      setTheme(newTheme);
      return;
    }

    const endRadius = Math.hypot(
      Math.max(x, innerWidth - x),
      Math.max(y, innerHeight - y),
    );

    const transition = document.startViewTransition(() => {
      flushSync(() => {
        setTheme(newTheme);
      });
    });

    transition.ready.then(() => {
      const clipPath = [
        `circle(0px at ${x}px ${y}px)`,
        `circle(${endRadius}px at ${x}px ${y}px)`,
      ];

      // If switching TO dark, we reveal the dark theme (new snapshot)
      // If switching TO light, we reveal the light theme (new snapshot)
      // The visual effect depends on which pseudo-element we animate.
      // Original logic:
      // isDark -> animate ::view-transition-new(root) with clipPath growing
      // !isDark -> animate ::view-transition-old(root) with clipPath shrinking (reverse)

      const isDark = newTheme === 'dark';

      document.documentElement.animate(
        {
          clipPath: isDark ? clipPath : [...clipPath].reverse(),
        },
        {
          duration: 400,
          easing: 'ease-in',
          pseudoElement: isDark
            ? '::view-transition-new(root)'
            : '::view-transition-old(root)',
          fill: 'forwards',
        },
      );
    });
  };

  return (
    <TooltipProvider disableHoverableContent>
      <Tooltip delayDuration={100}>
        <TooltipTrigger asChild>
          <Button
            className="rounded-full w-8 h-8 bg-background"
            variant="outline"
            size="icon"
            onClick={switchTheme}
          >
            <Sun className="w-[1.2rem] h-[1.2rem] rotate-90 scale-0 transition-transform ease-in-out duration-500 dark:rotate-0 dark:scale-100" />
            <Moon className="absolute w-[1.2rem] h-[1.2rem] rotate-0 scale-100 transition-transform ease-in-out duration-500 dark:-rotate-90 dark:scale-0" />
            <span className="sr-only">
              <Trans>Switch Theme</Trans>
            </span>
          </Button>
        </TooltipTrigger>
        <TooltipContent side="bottom">
          <Trans>Switch Theme</Trans>
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}
