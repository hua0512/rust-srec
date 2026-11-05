import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { FlagFormField } from '@/components/ui/flag-form-field';
import { Tabs, TabsContent, TabsList } from '@/components/ui/tabs';
import {
  ProcessorConfigFormProps,
  useProcessorFieldPath,
} from './common-props';
import {
  ProcessorFormShell,
  ProcessorNumberField,
  ProcessorSection,
  ProcessorSelectField,
  ProcessorTabTrigger,
  ProcessorTextField,
} from './processor-form-layout';
import { AudioExtractConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { Music, AudioWaveform, Settings2 } from 'lucide-react';

type AudioExtractConfig = z.infer<typeof AudioExtractConfigSchema>;

const AUDIO_FORMATS = [
  { value: 'mp3', label: 'MP3' },
  { value: 'aac', label: 'AAC' },
  { value: 'flac', label: 'FLAC' },
  { value: 'opus', label: 'Opus' },
];

export function AudioExtractConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps) {
  const { i18n } = useLingui();
  const fieldPath = useProcessorFieldPath<AudioExtractConfig>(pathPrefix);

  return (
    <ProcessorFormShell>
      <Tabs defaultValue="general" className="w-full">
        <TabsList className="w-full h-auto p-1.5 bg-muted/20 rounded-xl grid grid-cols-2 gap-1.5 backdrop-blur-sm mb-6">
          <ProcessorTabTrigger value="general" icon={Settings2}>
            <Trans>General</Trans>
          </ProcessorTabTrigger>
          <ProcessorTabTrigger value="advanced" icon={AudioWaveform}>
            <Trans>Advanced</Trans>
          </ProcessorTabTrigger>
        </TabsList>

        <div className="space-y-4">
          <TabsContent
            value="general"
            className="space-y-4 focus-visible:outline-none focus-visible:ring-0"
          >
            <ProcessorSection
              icon={Music}
              iconClassName="text-pink-500"
              title={<Trans>Output Settings</Trans>}
            >
              <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                <ProcessorSelectField
                  control={control}
                  name={fieldPath('format')}
                  label={<Trans>Format</Trans>}
                  placeholder={i18n._(msg`Copy (Stream Copy)`)}
                  description={<Trans>Leave empty for stream copy</Trans>}
                  options={AUDIO_FORMATS}
                />
                <ProcessorTextField
                  control={control}
                  name={fieldPath('bitrate')}
                  label={<Trans>Bitrate</Trans>}
                  description={<Trans>e.g. 128k, 320k</Trans>}
                  placeholder="128k"
                  className="font-mono text-sm"
                />
              </div>
            </ProcessorSection>
          </TabsContent>

          <TabsContent
            value="advanced"
            className="space-y-6 focus-visible:outline-none focus-visible:ring-0"
          >
            <ProcessorSection
              icon={AudioWaveform}
              iconClassName="text-indigo-500"
              title={<Trans>Audio Properties</Trans>}
            >
              <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                <ProcessorNumberField
                  control={control}
                  name={fieldPath('sample_rate')}
                  label={<Trans>Sample Rate (Hz)</Trans>}
                  step={1}
                  placeholder="44100"
                />
                <ProcessorNumberField
                  control={control}
                  name={fieldPath('channels')}
                  label={<Trans>Channels</Trans>}
                  min={1}
                  max={8}
                  step={1}
                  placeholder="2"
                />
              </div>
            </ProcessorSection>

            <FlagFormField
              fieldName={fieldPath('overwrite')}
              title={<Trans>Overwrite Files</Trans>}
              description={
                <Trans>Replace output files if they already exist</Trans>
              }
            />
          </TabsContent>
        </div>
      </Tabs>
    </ProcessorFormShell>
  );
}
