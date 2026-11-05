import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { FlagFormField } from '@/components/ui/flag-form-field';
import {
  ProcessorConfigFormProps,
  useProcessorFieldPath,
} from './common-props';
import {
  ProcessorFormShell,
  ProcessorNumberField,
  ProcessorSection,
  ProcessorSelectField,
  ProcessorTextField,
} from './processor-form-layout';
import { AssBurninConfigSchema, SOFTWARE_PRESETS } from '../processor-schemas';
import { z } from 'zod';
import { Video, Type, Settings, Trash2 } from 'lucide-react';

type AssBurninConfig = z.infer<typeof AssBurninConfigSchema>;

const MATCH_STRATEGIES = [
  { value: 'manifest', label: <Trans>Session pairing (same segment)</Trans> },
  { value: 'stem', label: <Trans>Stem (Filename Match)</Trans> },
];

export function AssBurninConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps) {
  const { i18n } = useLingui();
  const fieldPath = useProcessorFieldPath<AssBurninConfig>(pathPrefix);

  return (
    <ProcessorFormShell>
      <div className="space-y-6">
        <ProcessorSection
          icon={Video}
          iconClassName="text-blue-500"
          title={<Trans>Encoding Settings</Trans>}
        >
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <ProcessorTextField
              control={control}
              name={fieldPath('video_codec')}
              label={<Trans>Video Codec</Trans>}
              placeholder="libx264"
              className="font-mono text-sm"
            />
            <ProcessorTextField
              control={control}
              name={fieldPath('audio_codec')}
              label={<Trans>Audio Codec</Trans>}
              placeholder="copy"
              className="font-mono text-sm"
            />
            <ProcessorNumberField
              control={control}
              name={fieldPath('crf')}
              label={<Trans>CRF (0-51)</Trans>}
              min={0}
              max={51}
              step={1}
              placeholder="23"
            />
            <ProcessorSelectField
              control={control}
              name={fieldPath('preset')}
              label={<Trans>Encoder Preset</Trans>}
              placeholder="veryfast"
              options={SOFTWARE_PRESETS}
            />
          </div>
        </ProcessorSection>

        <ProcessorSection
          icon={Type}
          iconClassName="text-indigo-500"
          title={<Trans>Subtitle Matching</Trans>}
        >
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <ProcessorSelectField
              control={control}
              name={fieldPath('match_strategy')}
              label={<Trans>Match Strategy</Trans>}
              placeholder={i18n._(msg`Select strategy`)}
              description={
                <Trans>How to pair video files with .ass subtitles</Trans>
              }
              options={MATCH_STRATEGIES}
            />
            <FlagFormField
              fieldName={fieldPath('require_ass')}
              title={<Trans>Require Subtitles</Trans>}
            />
          </div>
        </ProcessorSection>

        <ProcessorSection
          icon={Settings}
          iconClassName="text-slate-500"
          title={<Trans>Advanced Settings</Trans>}
        >
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <ProcessorTextField
              control={control}
              name={fieldPath('ffmpeg_path')}
              label={<Trans>FFmpeg Path</Trans>}
              placeholder="ffmpeg"
              className="font-mono text-sm"
            />
            <ProcessorTextField
              control={control}
              name={fieldPath('fonts_dir')}
              label={<Trans>Fonts Directory</Trans>}
              placeholder="/usr/share/fonts"
              className="font-mono text-sm"
            />
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FlagFormField
              fieldName={fieldPath('passthrough_inputs')}
              title={<Trans>Passthrough Inputs</Trans>}
            />
            <FlagFormField
              fieldName={fieldPath('exclude_ass_from_passthrough')}
              title={<Trans>Exclude ASS from Passthrough</Trans>}
            />
          </div>
        </ProcessorSection>

        <ProcessorSection
          icon={Trash2}
          iconClassName="text-red-500"
          title={<Trans>Cleanup</Trans>}
        >
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FlagFormField
              fieldName={fieldPath('delete_source_videos_on_success')}
              title={<Trans>Delete Source Videos</Trans>}
            />
            <FlagFormField
              fieldName={fieldPath('delete_source_ass_on_success')}
              title={<Trans>Delete Source ASS</Trans>}
            />
          </div>
        </ProcessorSection>
      </div>
    </ProcessorFormShell>
  );
}
