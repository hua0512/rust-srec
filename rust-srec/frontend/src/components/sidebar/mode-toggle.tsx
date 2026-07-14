import { Moon, Sun } from 'lucide-react';
import { Trans } from '@lingui/react/macro';

import { Button } from '@/components/ui/button';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
  TooltipProvider,
} from '@/components/ui/tooltip';
import { useTheme } from '@/components/providers/theme-provider';
import { useCircularTransition } from '@/hooks/use-circular-transition';

export function ModeToggle() {
  const { resolvedMode } = useTheme();
  const { setModeWithReveal } = useCircularTransition();

  return (
    <TooltipProvider disableHoverableContent>
      <Tooltip delayDuration={100}>
        <TooltipTrigger asChild>
          <Button
            className="rounded-full w-8 h-8 bg-background"
            variant="outline"
            size="icon"
            onClick={(event) => {
              // Toggle from the resolved appearance: with mode 'system' the
              // preference itself never equals 'dark', so comparing `mode`
              // would set the appearance the user already sees.
              setModeWithReveal(resolvedMode === 'dark' ? 'light' : 'dark', {
                x: event.clientX,
                y: event.clientY,
              });
            }}
          >
            <Sun className="w-[1.2rem] h-[1.2rem] rotate-90 scale-95 opacity-0 transition-all ease-in-out duration-500 dark:rotate-0 dark:scale-100 dark:opacity-100" />
            <Moon className="absolute w-[1.2rem] h-[1.2rem] rotate-0 scale-100 opacity-100 transition-all ease-in-out duration-500 dark:-rotate-90 dark:scale-95 dark:opacity-0" />
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
