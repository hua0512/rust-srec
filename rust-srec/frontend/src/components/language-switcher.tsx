import { Languages } from 'lucide-react';
import { Button } from './ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from './ui/dropdown-menu';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
  TooltipProvider,
} from '@/components/ui/tooltip';
import { useLingui } from '@lingui/react';
import { Trans } from '@lingui/react/macro';
import { dynamicActivate, Locale } from '@/integrations/lingui/i18n';
import { updateLocale } from '@/server/functions/locale';
import { useRouter } from '@tanstack/react-router';

export function LanguageSwitcher() {
  const { i18n } = useLingui();
  const router = useRouter();

  const changeLocale = async (locale: Locale) => {
    // 1. Update the cookie and session on the server
    await updateLocale({ data: locale });
    // 2. Load and activate the new locale on the client
    await dynamicActivate(i18n, locale);
    // 3. Invalidate the router to refresh data if needed
    await router.invalidate();
  };

  return (
    <DropdownMenu>
      <TooltipProvider disableHoverableContent>
        <Tooltip delayDuration={100}>
          <TooltipTrigger asChild>
            <DropdownMenuTrigger asChild>
              <Button
                variant="outline"
                size="icon"
                className="rounded-full w-8 h-8 bg-background"
              >
                <Languages className="h-[1.2rem] w-[1.2rem]" />
                <span className="sr-only">
                  <Trans>Switch language</Trans>
                </span>
              </Button>
            </DropdownMenuTrigger>
          </TooltipTrigger>
          <TooltipContent side="bottom">
            <Trans>Switch language</Trans>
          </TooltipContent>
        </Tooltip>
      </TooltipProvider>
      <DropdownMenuContent align="end">
        <DropdownMenuItem onClick={() => changeLocale('en')}>
          English
        </DropdownMenuItem>
        <DropdownMenuItem onClick={() => changeLocale('zh-CN')}>
          中文 (简体)
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
