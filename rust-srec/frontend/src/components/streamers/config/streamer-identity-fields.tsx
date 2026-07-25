import { UseFormReturn, useWatch } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormMessage,
} from '@/components/ui/form';
import { IconInput } from '@/components/ui/icon-input';
import { Button } from '@/components/ui/button';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import {
  AlertTriangle,
  Link,
  Loader2,
  Radio,
  Sparkles,
  Tv,
  User,
} from 'lucide-react';
import { usePlatformDetection } from '@/hooks/use-platform-detection';
import {
  CONFIG_DESCRIPTION,
  CONFIG_INPUT,
  ConfigFieldLabel,
  ConfigSectionHeading,
} from '@/components/config/shared/config-field';
import { cn } from '@/lib/utils';

interface StreamerIdentityFieldsProps {
  form: UseFormReturn<any>;
  onAutofillName?: () => void;
  isAutofilling?: boolean;
}

/**
 * What identifies a streamer: its URL, its display name, and the platform derived from the URL.
 *
 * Shared by the create wizard's first step and the edit page's General tab so the two never drift
 * in styling or validation.
 */
export function StreamerIdentityFields({
  form,
  onAutofillName,
  isAutofilling = false,
}: StreamerIdentityFieldsProps) {
  const { i18n } = useLingui();
  const url = useWatch({ control: form.control, name: 'url' });
  const { platform, isDetecting, isUnsupported } = usePlatformDetection(url);

  return (
    <section className="space-y-4">
      <ConfigSectionHeading icon={Tv}>
        <Trans>Stream Details</Trans>
      </ConfigSectionHeading>

      <FormField
        control={form.control}
        name="url"
        render={({ field }) => (
          <FormItem className="space-y-2">
            <ConfigFieldLabel>
              <Trans>URL</Trans>
            </ConfigFieldLabel>
            <FormControl>
              <IconInput
                icon={Link}
                placeholder={i18n._(msg`https://twitch.tv/...`)}
                {...field}
                className={cn(CONFIG_INPUT, 'font-mono text-sm')}
              />
            </FormControl>
            <FormDescription className={CONFIG_DESCRIPTION}>
              <Trans>The direct link to the channel or stream.</Trans>
            </FormDescription>
            <FormMessage />
          </FormItem>
        )}
      />

      <FormField
        control={form.control}
        name="name"
        render={({ field }) => (
          <FormItem className="space-y-2">
            <ConfigFieldLabel>
              <Trans>Name</Trans>
            </ConfigFieldLabel>
            <FormControl>
              <div className="flex gap-2">
                <IconInput
                  icon={User}
                  placeholder={i18n._(msg`e.g. My Favorite Streamer`)}
                  {...field}
                  className={cn(CONFIG_INPUT, 'flex-1')}
                />
                {onAutofillName && (
                  <TooltipProvider>
                    <Tooltip>
                      <TooltipTrigger asChild>
                        <Button
                          type="button"
                          variant="outline"
                          size="icon"
                          className="h-11 w-11 shrink-0 rounded-xl border-border/50 shadow-sm"
                          onClick={onAutofillName}
                          disabled={isAutofilling || !url}
                        >
                          {isAutofilling ? (
                            <Loader2 className="h-4 w-4 animate-spin" />
                          ) : (
                            <Sparkles className="h-4 w-4" />
                          )}
                          <span className="sr-only">
                            <Trans>Autofill name from URL</Trans>
                          </span>
                        </Button>
                      </TooltipTrigger>
                      <TooltipContent>
                        <Trans>Autofill name from URL</Trans>
                      </TooltipContent>
                    </Tooltip>
                  </TooltipProvider>
                )}
              </div>
            </FormControl>
            <FormMessage />
          </FormItem>
        )}
      />

      <PlatformReadout
        platform={platform}
        isDetecting={isDetecting}
        isUnsupported={isUnsupported}
      />
    </section>
  );
}

/**
 * Read-only display of the platform the backend will derive from the URL.
 *
 * Not a form field: `platform_config_id` is server-derived, so showing a picker here would imply
 * a choice the API does not accept.
 */
function PlatformReadout({
  platform,
  isDetecting,
  isUnsupported,
}: {
  platform: string | null;
  isDetecting: boolean;
  isUnsupported: boolean;
}) {
  return (
    <div className="space-y-2">
      <ConfigFieldLabel plain>
        <Trans>Platform</Trans>
      </ConfigFieldLabel>
      <div
        className="flex h-11 items-center gap-2 rounded-xl border border-border/50 bg-muted/40 px-3 text-sm shadow-sm"
        aria-live="polite"
      >
        {isDetecting ? (
          <>
            <Loader2 className="h-4 w-4 shrink-0 animate-spin text-muted-foreground" />
            <span className="text-muted-foreground">
              <Trans>Detecting…</Trans>
            </span>
          </>
        ) : platform ? (
          <>
            <Radio className="h-4 w-4 shrink-0 text-primary" />
            <span className="font-medium">{platform}</span>
          </>
        ) : isUnsupported ? (
          <>
            <AlertTriangle className="h-4 w-4 shrink-0 text-destructive" />
            <span className="text-destructive">
              <Trans>Unsupported link</Trans>
            </span>
          </>
        ) : (
          <span className="text-muted-foreground">
            <Trans>Enter a URL above</Trans>
          </span>
        )}
      </div>
      <p className={CONFIG_DESCRIPTION}>
        {isUnsupported ? (
          <Trans>
            No platform recognizes this link, so it cannot be recorded.
          </Trans>
        ) : (
          <Trans>
            Determined from the URL. Platform settings apply automatically.
          </Trans>
        )}
      </p>
    </div>
  );
}
