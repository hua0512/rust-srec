import type { FieldValues, Path, UseFormReturn } from 'react-hook-form';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Activity, Shield } from 'lucide-react';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
} from '@/components/ui/form';
import { Switch } from '@/components/ui/switch';
import { EndStreamOnDanmuCloseField } from '@/components/config/shared/end-stream-on-danmu-close-field';
import { ConfigSectionHeading } from '@/components/config/shared/config-field';
import { configPath } from '@/components/config/shared/form-path';
import { TwitchCodecValues } from '@/api/schemas/platform-configs';

type TwitchCodec = (typeof TwitchCodecValues)[number];
type OptionalCodec = Exclude<TwitchCodec, 'h264'>;

interface TwitchConfigFieldsProps<TFieldValues extends FieldValues> {
  form: UseFormReturn<TFieldValues>;
  /** Path to the object holding this platform's options. */
  fieldName: Path<TFieldValues>;
}

/**
 * The codec list with `codec` switched on or off. H.264 is always announced,
 * so every stream keeps a fallback, and the list stays in the order of
 * `TwitchCodecValues`.
 */
function toggleCodec(
  current: readonly TwitchCodec[] | null | undefined,
  codec: OptionalCodec,
  enabled: boolean,
): TwitchCodec[] {
  const selected = new Set<TwitchCodec>(current ?? []);
  selected.add('h264');
  if (enabled) {
    selected.add(codec);
  } else {
    selected.delete(codec);
  }
  return TwitchCodecValues.filter((value) => selected.has(value));
}

const OPTIONAL_CODECS = [
  {
    codec: 'h265',
    label: msg`Allow H.265 (HEVC)`,
    description: msg`Record the qualities some channels stream only in H.265. H.264 qualities stay available. Turn on only if your players and post-processing handle H.265.`,
  },
  {
    codec: 'av1',
    label: msg`Allow AV1`,
    description: msg`Record the qualities some channels stream only in AV1. H.264 qualities stay available. Turn on only if your players and post-processing handle AV1.`,
  },
] as const;

/** The OAuth token belongs to a Twitch account profile, not to these options. */
export function TwitchConfigFields<TFieldValues extends FieldValues>({
  form,
  fieldName,
}: TwitchConfigFieldsProps<TFieldValues>) {
  const { i18n } = useLingui();

  return (
    <div className="space-y-12">
      <section className="space-y-6">
        <ConfigSectionHeading icon={Activity} accent="indigo">
          <Trans>Quality Settings</Trans>
        </ConfigSectionHeading>

        <FormField
          control={form.control}
          name={configPath<TFieldValues>(fieldName, 'supported_codecs')}
          render={({ field }) => {
            const value = field.value as TwitchCodec[] | null | undefined;
            return (
              <div className="grid gap-6">
                {OPTIONAL_CODECS.map(({ codec, label, description }) => (
                  <FormItem
                    key={codec}
                    className="flex flex-row items-center justify-between rounded-2xl border bg-muted/5 p-5 transition-all hover:bg-muted/10 border-border/50"
                  >
                    <div className="space-y-1.5 pr-4">
                      <FormLabel className="text-sm font-bold text-foreground">
                        {i18n._(label)}
                      </FormLabel>
                      <FormDescription className="text-xs leading-relaxed font-medium">
                        {i18n._(description)}
                      </FormDescription>
                    </div>
                    <FormControl>
                      <Switch
                        checked={!!value?.includes(codec)}
                        onCheckedChange={(enabled) =>
                          field.onChange(toggleCodec(value, codec, enabled))
                        }
                      />
                    </FormControl>
                  </FormItem>
                ))}
              </div>
            );
          }}
        />
      </section>

      <section className="space-y-6">
        <ConfigSectionHeading icon={Shield} accent="indigo">
          <Trans>Danmu Control</Trans>
        </ConfigSectionHeading>
        <EndStreamOnDanmuCloseField
          form={form}
          name={configPath<TFieldValues>(
            fieldName,
            'end_stream_on_danmu_stream_closed',
          )}
        />
      </section>
    </div>
  );
}
