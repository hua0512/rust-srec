import { msg } from '@lingui/core/macro';
import { Clock, SquareTerminal, Terminal } from 'lucide-react';
import { FfmpegConfigOverrideSchema } from '@/api/schemas/engine';
import { LosslessCuttingField } from './lossless-cutting-field';
import { OverrideScope } from './setting-fields';
import {
  AdvancedSections,
  SettingsSection,
  type SectionSpec,
} from './setting-sections';

/* FFmpeg's settings; each field's `fallback` is the engine default. */

const FFMPEG_SECTIONS: readonly SectionSpec[] = [
  {
    id: 'ffmpeg-program',
    label: msg`Program`,
    icon: Terminal,
    fields: [
      {
        kind: 'text',
        path: 'binary_path',
        label: msg`Binary Path`,
        description: msg`Absolute path or 'ffmpeg' if in PATH`,
        fallback: 'ffmpeg',
      },
      {
        kind: 'text',
        path: 'user_agent',
        label: msg`User Agent`,
        description: msg`Custom User-Agent string (Optional)`,
        fallback: '',
        fallbackLabel: msg`None`,
      },
    ],
  },
  {
    id: 'ffmpeg-arguments',
    label: msg`Arguments`,
    icon: SquareTerminal,
    fields: [
      {
        kind: 'list',
        path: 'input_args',
        label: msg`Input Arguments`,
        description: msg`Args inserted before -i input_url`,
        placeholder: '-reconnect 1',
        fallback: [],
      },
      {
        kind: 'list',
        path: 'output_args',
        label: msg`Output Arguments`,
        description: msg`Args used for processing/encoding`,
        placeholder: '-c copy',
        fallback: [],
      },
    ],
  },
];

const FFMPEG_ADVANCED: readonly SectionSpec[] = [
  {
    id: 'ffmpeg-timeouts',
    label: msg`Timeouts`,
    icon: Clock,
    fields: [
      {
        kind: 'duration',
        path: 'timeout_secs',
        label: msg`Timeout`,
        description: msg`Connection/activity timeout`,
        fallback: 30,
      },
      {
        kind: 'duration',
        path: 'graceful_stop_timeout_secs',
        label: msg`Graceful Stop Timeout`,
        description: msg`Time to wait for ffmpeg to finalize the file before force-killing it`,
        fallback: 60,
      },
    ],
  },
];

const NO_ENGINE_CONFIG = {};

interface FfmpegFormProps {
  basePath?: string;
  /** Edit a template's override of an engine instead of the engine itself. */
  isOverride?: boolean;
  /** The overridden engine's configuration, whose values unset override fields inherit. */
  engineConfig?: Record<string, unknown>;
}

export function FfmpegForm({
  basePath = 'config',
  isOverride = false,
  engineConfig,
}: FfmpegFormProps) {
  return (
    <OverrideScope
      isOverride={isOverride}
      basePath={basePath}
      engineConfig={engineConfig ?? NO_ENGINE_CONFIG}
      schema={FfmpegConfigOverrideSchema}
    >
      <div className="space-y-6">
        <LosslessCuttingField basePath={basePath} argsField="output_args" />
        {FFMPEG_SECTIONS.map((section) => (
          <SettingsSection
            key={section.id}
            section={section}
            basePath={basePath}
          />
        ))}
        <AdvancedSections sections={FFMPEG_ADVANCED} basePath={basePath} />
      </div>
    </OverrideScope>
  );
}
