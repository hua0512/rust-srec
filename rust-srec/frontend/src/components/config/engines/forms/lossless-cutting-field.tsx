import { Trans } from '@lingui/react/macro';
import { Info } from 'lucide-react';

import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
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

export function LosslessCuttingField({
  basePath,
  isOverride = false,
}: {
  basePath: string;
  isOverride?: boolean;
}) {
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
          <Alert role="note">
            <Info />
            <AlertTitle>
              <Trans>Recording uses temporary chunks</Trans>
            </AlertTitle>
            <AlertDescription>
              <p>
                <Trans>
                  When enabled, recordings are written to temporary chunks and
                  combined into final files after a cut, a size or duration
                  limit, or recording stops. Final files are available only
                  after this step finishes.
                </Trans>
              </p>
              <p>
                <Trans>
                  Uses extra disk space and I/O. Keep temporary chunks if file
                  finalization fails.
                </Trans>
              </p>
              <p>
                <Trans>
                  Requires MP4, MKV, FLV, TS, or MOV output with no custom
                  FFmpeg output arguments or extra Streamlink arguments. Other
                  settings use normal recording without lossless cutting.
                </Trans>
              </p>
            </AlertDescription>
          </Alert>
          <FormMessage />
        </FormItem>
      )}
    />
  );
}
