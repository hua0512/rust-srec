import { useId } from 'react';
import { Cpu, Film, Gauge, Server, type LucideIcon } from 'lucide-react';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import type { MessageDescriptor } from '@lingui/core';
import { formatBitrate } from '@/lib/format';

import { OptionGroup, SettingsSection } from './option-group';
import {
  extractStreams,
  selectStreamLevel,
  streamLevelKey,
  streamLevelOptions,
  streamLevels,
  type StreamLevel,
  type StreamOption,
} from './stream-source';
export type { StreamOption } from './stream-source';

export interface StreamInfoCardProps {
  mediaInfo: any;
  selectedStream: StreamOption | null;
  onStreamSelect: (stream: StreamOption) => void;
}

const levelSections: Record<
  StreamLevel,
  { title: MessageDescriptor; icon: LucideIcon }
> = {
  format: { title: msg`Format`, icon: Film },
  cdn: { title: msg`CDN`, icon: Server },
  quality: { title: msg`Quality`, icon: Gauge },
  variant: { title: msg`Codec`, icon: Cpu },
};

export function StreamInfoCard({
  mediaInfo,
  selectedStream,
  onStreamSelect,
}: StreamInfoCardProps) {
  const { i18n } = useLingui();
  const baseId = useId();
  const streams = extractStreams(mediaInfo);
  const selected = selectedStream ?? streams[0];
  if (!selected) return null;

  const fallback = i18n._(msg`Default`);
  const optionLabel = (level: StreamLevel, stream: StreamOption) => {
    switch (level) {
      case 'format':
        return stream.format?.toUpperCase() || fallback;
      case 'cdn':
        return stream.cdn || fallback;
      case 'quality':
        return stream.quality || fallback;
      case 'variant':
        return (
          [stream.codec?.toUpperCase(), stream.container?.toUpperCase()]
            .filter(Boolean)
            .join(' · ') || fallback
        );
    }
  };

  const sections = streamLevels
    .map((level) => ({
      level,
      options: streamLevelOptions(streams, selected, level),
    }))
    .filter(({ options }) => options.length > 1);
  const details = [
    selected.format?.toUpperCase(),
    selected.codec,
    selected.container?.toUpperCase(),
    selected.fps ? `${selected.fps} fps` : undefined,
    formatBitrate(selected.bitrate),
  ].filter((detail, index, all) => detail && all.indexOf(detail) === index);

  return (
    <div className="space-y-5">
      {sections.map(({ level, options }) => {
        const id = `${baseId}-${level}`;
        const { title, icon } = levelSections[level];
        return (
          <SettingsSection
            key={level}
            id={id}
            icon={icon}
            title={i18n._(title)}
            aside={options.length}
          >
            <OptionGroup
              aria-labelledby={id}
              columns={options.length === 2 || options.length === 4 ? 2 : 3}
              value={streamLevelKey(selected, level)}
              onValueChange={(value) => {
                const stream = selectStreamLevel(
                  streams,
                  selected,
                  level,
                  value,
                );
                if (stream) onStreamSelect(stream);
              }}
              options={options.map(({ value, stream }) => ({
                value,
                label: optionLabel(level, stream),
                hint:
                  level === 'quality'
                    ? formatBitrate(stream.bitrate)
                    : undefined,
              }))}
            />
          </SettingsSection>
        );
      })}

      {details.length > 0 && (
        <div className="flex flex-wrap gap-1.5">
          {details.map((detail) => (
            <span
              key={detail}
              className="rounded-md border border-border/60 bg-muted/40 px-2 py-0.5 text-[10px] font-medium text-muted-foreground tabular-nums"
            >
              {detail}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
