import type { FieldValues, Path, UseFormReturn } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
} from '@/components/ui/form';
import { Trans } from '@lingui/react/macro';
import { FlagFormField } from '@/components/ui/flag-form-field';
import { Zap, Shield } from 'lucide-react';
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { EndStreamOnDanmuCloseField } from '@/components/config/shared/end-stream-on-danmu-close-field';
import {
  ConfigFieldLabel,
  ConfigSectionHeading,
} from '@/components/config/shared/config-field';
import { configPath } from '@/components/config/shared/form-path';

interface DouyinConfigFieldsProps<TFieldValues extends FieldValues> {
  form: UseFormReturn<TFieldValues>;
  /** Path to the object holding this platform's options. */
  fieldName: Path<TFieldValues>;
}

export function DouyinConfigFields<TFieldValues extends FieldValues>({
  form,
  fieldName,
}: DouyinConfigFieldsProps<TFieldValues>) {
  return (
    <div className="space-y-12">
      {/* Extraction Settings Section */}
      <section className="space-y-6">
        <ConfigSectionHeading icon={Zap} accent="indigo">
          <Trans>Extraction Settings</Trans>
        </ConfigSectionHeading>

        <div className="grid gap-6">
          <FlagFormField
            fieldName={configPath<TFieldValues>(
              fieldName,
              'force_origin_quality',
            )}
            title={<Trans>Force Origin Quality</Trans>}
            description={
              <Trans>
                Attempts to get origin quality by replacing the audio stream.
                May result in audio-only streams if it fails.
              </Trans>
            }
            checked={(value) => !!value}
            showExperimentalBadge
          />

          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <FlagFormField
              fieldName={configPath<TFieldValues>(fieldName, 'double_screen')}
              title={<Trans>Double Screen Data</Trans>}
              description={<Trans>Capture separate stream data.</Trans>}
              checked={(value) => !!value}
            />

            <FlagFormField
              fieldName={configPath<TFieldValues>(
                fieldName,
                'force_mobile_api',
              )}
              title={<Trans>Force Mobile API</Trans>}
              description={<Trans>Use mobile endpoint for extraction.</Trans>}
              checked={(value) => !!value}
            />
          </div>

          <FlagFormField
            fieldName={configPath<TFieldValues>(
              fieldName,
              'skip_interactive_games',
            )}
            title={<Trans>Skip Interactive Games</Trans>}
            description={
              <Trans>
                Treat interactive game streams (互动玩法) as offline.
              </Trans>
            }
            checked={(value) => !!value}
          />
        </div>
      </section>

      {/* Security & Identity Section */}
      <section className="space-y-6">
        <ConfigSectionHeading icon={Shield} accent="indigo">
          <Trans>Security & Identity</Trans>
        </ConfigSectionHeading>

        <div className="space-y-6">
          <FormField
            control={form.control}
            name={configPath<TFieldValues>(fieldName, 'ttwid_management_mode')}
            render={({ field }) => (
              <FormItem>
                <ConfigFieldLabel accent="indigo" className="mb-3">
                  <Trans>TTWID Management Mode</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <Tabs
                    onValueChange={field.onChange}
                    value={field.value || 'global'}
                    className="w-full"
                  >
                    <TabsList className="grid w-full grid-cols-2 bg-muted/30 p-1 rounded-xl h-11 border border-border/40">
                      <TabsTrigger
                        value="global"
                        className="rounded-lg data-[state=active]:bg-background data-[state=active]:shadow-sm font-bold text-xs uppercase tracking-wide"
                      >
                        <Trans>Global</Trans>
                      </TabsTrigger>
                      <TabsTrigger
                        value="per-streamer"
                        className="rounded-lg data-[state=active]:bg-background data-[state=active]:shadow-sm font-bold text-xs uppercase tracking-wide"
                      >
                        <Trans>Per-Streamer</Trans>
                      </TabsTrigger>
                    </TabsList>
                  </Tabs>
                </FormControl>
                <FormDescription className="text-[11px] font-medium pt-2 px-1">
                  <Trans>
                    Isolation level for tracking identifiers. Global is usually
                    best.
                  </Trans>
                </FormDescription>
              </FormItem>
            )}
          />
        </div>
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
