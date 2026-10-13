import { msg } from '@lingui/core/macro';
import { Radio, Terminal, TimerOff, Tv } from 'lucide-react';
import { StreamlinkConfigOverrideSchema } from '@/api/schemas/engine';
import { LosslessCuttingField } from './lossless-cutting-field';
import { OverrideScope } from './setting-fields';
import {
  AdvancedSections,
  SettingsSection,
  type SectionSpec,
} from './setting-sections';

/* Streamlink's settings; each field's `fallback` is the engine default. */

const STREAMLINK_SECTIONS: readonly SectionSpec[] = [
  {
    id: 'streamlink-recording',
    label: msg({ message: 'Recording', context: 'engine settings section' }),
    icon: Radio,
    fields: [
      {
        kind: 'text',
        path: 'quality',
        label: msg`Quality`,
        description: msg`e.g. 'best', 'worst', '720p', 'audio_only'`,
        fallback: 'best',
      },
      {
        kind: 'list',
        path: 'extra_args',
        label: msg`Extra Arguments`,
        description: msg`Any additional command line arguments to pass to Streamlink`,
        placeholder: '--hls-live-edge 3',
        fallback: [],
      },
    ],
  },
  {
    id: 'streamlink-programs',
    label: msg`Programs`,
    icon: Terminal,
    fields: [
      {
        kind: 'text',
        path: 'binary_path',
        label: msg`Binary Path`,
        description: msg`Absolute path or 'streamlink' in PATH`,
        fallback: 'streamlink',
      },
      {
        kind: 'text',
        path: 'ffmpeg_path',
        label: msg`FFmpeg Path`,
        description: msg`FFmpeg used to remux Streamlink output. Enter a path on the backend server without shell quotes. The environment default uses FFMPEG_PATH, then ffmpeg from PATH.`,
        fallback: '',
        fallbackLabel: 'FFMPEG_PATH or ffmpeg',
      },
    ],
  },
];

const STREAMLINK_ADVANCED: readonly SectionSpec[] = [
  {
    id: 'streamlink-twitch',
    label: msg`Twitch (ttv-lol)`,
    icon: Tv,
    fields: [
      {
        kind: 'text',
        path: 'twitch_proxy_playlist',
        label: msg`Twitch Proxy Playlist`,
        description: msg`Custom proxy URL for Twitch playlists`,
        placeholder: 'https://lb-eu.cdn-perfprod.com',
        fallback: '',
        fallbackLabel: msg`None`,
      },
      {
        kind: 'text',
        path: 'twitch_proxy_playlist_exclude',
        label: msg`Twitch Proxy Playlist Exclude`,
        description: msg`Comma-separated list of channels to exclude from proxy`,
        placeholder: 'channel1,channel2',
        fallback: '',
        fallbackLabel: msg`None`,
      },
    ],
  },
  {
    id: 'streamlink-stopping',
    label: msg`Stopping`,
    icon: TimerOff,
    fields: [
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

interface StreamlinkFormProps {
  basePath?: string;
  /** Edit a template's override of an engine instead of the engine itself. */
  isOverride?: boolean;
  /** The overridden engine's configuration, whose values unset override fields inherit. */
  engineConfig?: Record<string, unknown>;
}

export function StreamlinkForm({
  basePath = 'config',
  isOverride = false,
  engineConfig,
}: StreamlinkFormProps) {
  return (
    <OverrideScope
      isOverride={isOverride}
      basePath={basePath}
      engineConfig={engineConfig ?? NO_ENGINE_CONFIG}
      schema={StreamlinkConfigOverrideSchema}
    >
      <div className="space-y-6">
        <LosslessCuttingField basePath={basePath} argsField="extra_args" />
        {STREAMLINK_SECTIONS.map((section) => (
          <SettingsSection
            key={section.id}
            section={section}
            basePath={basePath}
          />
        ))}
        <AdvancedSections sections={STREAMLINK_ADVANCED} basePath={basePath} />
      </div>
    </OverrideScope>
  );
}
