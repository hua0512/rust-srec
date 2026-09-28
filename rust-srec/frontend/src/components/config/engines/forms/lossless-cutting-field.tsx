import { Trans } from '@lingui/react/macro';
import { Info, TriangleAlert } from 'lucide-react';
import { useWatch } from 'react-hook-form';

import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import {
  Disclosure,
  DisclosureContent,
  DisclosureTrigger,
} from '@/components/ui/disclosure';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Switch } from '@/components/ui/switch';

/**
 * The engine's custom-arguments field that disables chunked recording when it
 * is non-empty. Mirrors the gate in the backend FFmpeg and Streamlink engines;
 * the output format half of that gate lives in the recording configuration,
 * not in engine settings, so it can only be described here.
 */
export type LosslessCuttingArgsField = 'output_args' | 'extra_args';

export function LosslessCuttingField({
  basePath,
  argsField,
  isOverride = false,
}: {
  basePath: string;
  argsField: LosslessCuttingArgsField;
  isOverride?: boolean;
}) {
  const enabled = useWatch({ name: `${basePath}.enable_lossless_cutting` });
  // An override without this key inherits the engine's arguments, which are
  // not known here; only arguments set in this form are evaluated.
  const args: unknown = useWatch({ name: `${basePath}.${argsField}` });
  const blockedByArgs = Array.isArray(args) && args.length > 0;
  // Overrides cannot see the inherited engine value, so details follow only
  // an explicit `true`.
  const showDetails = enabled === true;

  return (
    <FormField
      name={`${basePath}.enable_lossless_cutting`}
      render={({ field }) => (
        <FormItem className="rounded-xl border border-border/40 bg-background/50 p-4 space-y-3">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-2">
              <FormLabel>
                <Trans>Enable lossless cutting</Trans>
              </FormLabel>
              <Badge variant="secondary">
                <Trans>Experimental</Trans>
              </Badge>
            </div>
            {isOverride ? (
              <Select
                value={
                  field.value === undefined ? 'inherit' : String(field.value)
                }
                onValueChange={(value) =>
                  field.onChange(
                    value === 'inherit' ? undefined : value === 'true',
                  )
                }
              >
                <FormControl>
                  <SelectTrigger className="w-full sm:w-52">
                    <SelectValue />
                  </SelectTrigger>
                </FormControl>
                <SelectContent>
                  <SelectItem value="inherit">
                    <Trans>Use engine setting</Trans>
                  </SelectItem>
                  <SelectItem value="true">
                    <Trans>Enabled</Trans>
                  </SelectItem>
                  <SelectItem value="false">
                    <Trans>Disabled</Trans>
                  </SelectItem>
                </SelectContent>
              </Select>
            ) : (
              <FormControl>
                <Switch
                  checked={field.value ?? false}
                  onCheckedChange={field.onChange}
                />
              </FormControl>
            )}
          </div>
          <FormDescription>
            <Trans>
              Split recordings without reconnecting or re-encoding. Off by
              default. Changes apply to new recordings.
            </Trans>
          </FormDescription>
          {showDetails && blockedByArgs && (
            <Alert
              role="alert"
              className="bg-amber-500/5 border-amber-500/30 text-amber-700 dark:text-amber-400 *:data-[slot=alert-description]:text-amber-700/90 dark:*:data-[slot=alert-description]:text-amber-400/90"
            >
              <TriangleAlert />
              <AlertTitle className="line-clamp-none">
                <Trans>Lossless cutting will not be used</Trans>
              </AlertTitle>
              <AlertDescription>
                {argsField === 'output_args' ? (
                  <Trans>
                    Custom FFmpeg output arguments are set. Remove them to
                    record with lossless cutting; until then this engine records
                    normally.
                  </Trans>
                ) : (
                  <Trans>
                    Extra Streamlink arguments are set. Remove them to record
                    with lossless cutting; until then this engine records
                    normally.
                  </Trans>
                )}
              </AlertDescription>
            </Alert>
          )}
          {showDetails && <LosslessCuttingDetails />}
          <FormMessage />
        </FormItem>
      )}
    />
  );
}

function LosslessCuttingDetails() {
  return (
    <Disclosure className="rounded-lg border bg-card text-sm text-card-foreground">
      <DisclosureTrigger
        icon={<Info aria-hidden="true" />}
        className="rounded-lg px-4 py-3 tracking-tight"
      >
        <Trans>Recording uses temporary chunks</Trans>
      </DisclosureTrigger>
      <DisclosureContent className="grid gap-1 pr-4 pb-3 pl-11 text-muted-foreground [&_p]:leading-relaxed">
        <p>
          <Trans>
            Recordings are written to temporary chunks and combined into final
            files after a cut, a size or duration limit, or recording stops.
            Final files are available only after this step finishes.
          </Trans>
        </p>
        <p>
          <Trans>
            Uses extra disk space and I/O. Temporary chunks are kept if file
            finalization fails.
          </Trans>
        </p>
        <p>
          <Trans>
            Applies only to MP4, MKV, FLV, TS, or MOV output. Recordings in
            other formats use normal recording.
          </Trans>
        </p>
      </DisclosureContent>
    </Disclosure>
  );
}
