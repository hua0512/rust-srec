import { useId, useState, type ReactNode } from 'react';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Trans } from '@lingui/react/macro';
import { X } from 'lucide-react';
import { Slider } from '@/components/ui/slider';
import { Switch } from '@/components/ui/switch';
import { cn } from '@/lib/utils';
import {
  BLOCK_ENTRY_MAX_LENGTH,
  BLOCK_LIST_MAX_ENTRIES,
  DANMU_AREAS,
  DANMU_SPEED_SECS,
  defaultDanmuPreferences,
  type DanmuPreferences,
} from './danmu-preferences';

type Hidden = DanmuPreferences['hidden'];
type BlockKind = 'keyword' | 'user';

const HIDDEN_TYPES: { key: keyof Hidden; label: ReturnType<typeof msg> }[] = [
  { key: 'scroll', label: msg({ message: 'Scrolling', context: 'danmu' }) },
  { key: 'top', label: msg({ message: 'Top', context: 'danmu' }) },
  { key: 'bottom', label: msg({ message: 'Bottom', context: 'danmu' }) },
  { key: 'colored', label: msg({ message: 'Coloured', context: 'danmu' }) },
  {
    key: 'superchat',
    label: msg({ message: 'Super chats', context: 'danmu' }),
  },
];

const AREA_LABELS = [
  msg({ message: '1/4', context: 'danmu area' }),
  msg({ message: '1/2', context: 'danmu area' }),
  msg({ message: '3/4', context: 'danmu area' }),
  msg({ message: 'Full', context: 'danmu area' }),
];

const SPEED_LABELS = [
  msg({ message: 'Very slow', context: 'danmu speed' }),
  msg({ message: 'Slow', context: 'danmu speed' }),
  msg({ message: 'Normal', context: 'danmu speed' }),
  msg({ message: 'Fast', context: 'danmu speed' }),
  msg({ message: 'Very fast', context: 'danmu speed' }),
];

const BLOCK_PLACEHOLDER = {
  keyword: msg`Word to hide`,
  user: msg`Exact user name`,
};

function Section({
  title,
  children,
}: {
  title: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="space-y-2">
      <h3 className="text-xs font-medium text-muted-foreground">{title}</h3>
      {children}
    </section>
  );
}

function Chip({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-pressed={active}
      onClick={onClick}
      className={cn(
        'rounded-md border px-2 py-1 text-xs transition-colors',
        active
          ? 'border-primary bg-primary text-primary-foreground'
          : 'border-border bg-transparent hover:bg-accent',
      )}
    >
      {children}
    </button>
  );
}

function SliderRow({
  label,
  value,
  display,
  min,
  max,
  step,
  onChange,
}: {
  label: ReactNode;
  value: number;
  display: string;
  min: number;
  max: number;
  step: number;
  onChange: (value: number) => void;
}) {
  return (
    <Section title={label}>
      <div className="flex items-center gap-3">
        <Slider
          value={[value]}
          min={min}
          max={max}
          step={step}
          onValueChange={([next]) => onChange(next)}
        />
        <span className="w-16 shrink-0 text-right text-xs tabular-nums">
          {display}
        </span>
      </div>
    </Section>
  );
}

function SwitchRow({
  label,
  checked,
  onChange,
}: {
  label: ReactNode;
  checked: boolean;
  onChange: (checked: boolean) => void;
}) {
  const id = useId();
  return (
    <div className="flex items-center justify-between gap-3">
      <label htmlFor={id} className="text-sm">
        {label}
      </label>
      <Switch id={id} checked={checked} onCheckedChange={onChange} />
    </div>
  );
}

export interface DanmuSettingsPanelProps {
  preferences: DanmuPreferences;
  onChange: (preferences: DanmuPreferences) => void;
}

/**
 * The player's danmu settings: what to hide, how comments look and move, and
 * a blocklist of words and senders.
 */
export function DanmuSettingsPanel({
  preferences,
  onChange,
}: DanmuSettingsPanelProps) {
  const { i18n } = useLingui();
  const [blockKind, setBlockKind] = useState<BlockKind>('keyword');
  const [entry, setEntry] = useState('');
  const update = (patch: Partial<DanmuPreferences>) =>
    onChange({ ...preferences, ...patch });

  const listKey = blockKind === 'keyword' ? 'blockedKeywords' : 'blockedUsers';
  const normalized =
    blockKind === 'keyword' ? entry.trim().toLowerCase() : entry.trim();
  const canAdd =
    normalized.length > 0 &&
    !preferences[listKey].includes(normalized) &&
    preferences[listKey].length < BLOCK_LIST_MAX_ENTRIES;
  const addEntry = () => {
    if (!canAdd) return;
    update({ [listKey]: [...preferences[listKey], normalized] });
    setEntry('');
  };
  const removeEntry = (key: typeof listKey, value: string) =>
    update({ [key]: preferences[key].filter((item) => item !== value) });

  const blocked = [
    ...preferences.blockedKeywords.map((value) => ({
      key: 'blockedKeywords' as const,
      value,
    })),
    ...preferences.blockedUsers.map((value) => ({
      key: 'blockedUsers' as const,
      value,
    })),
  ];

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h2 className="text-sm font-semibold">
          <Trans>Danmu settings</Trans>
        </h2>
        <button
          type="button"
          className="text-xs text-muted-foreground hover:text-foreground"
          onClick={() =>
            onChange({
              ...defaultDanmuPreferences(),
              visible: preferences.visible,
              blockedKeywords: preferences.blockedKeywords,
              blockedUsers: preferences.blockedUsers,
            })
          }
        >
          <Trans context="danmu">Reset</Trans>
        </button>
      </div>

      <SwitchRow
        label={<Trans>Show danmu</Trans>}
        checked={preferences.visible}
        onChange={(visible) => update({ visible })}
      />

      <Section title={<Trans context="danmu">Hide</Trans>}>
        <div className="flex flex-wrap gap-1.5">
          {HIDDEN_TYPES.map(({ key, label }) => (
            <Chip
              key={key}
              active={preferences.hidden[key]}
              onClick={() =>
                update({
                  hidden: {
                    ...preferences.hidden,
                    [key]: !preferences.hidden[key],
                  },
                })
              }
            >
              {i18n._(label)}
            </Chip>
          ))}
        </div>
      </Section>

      <SliderRow
        label={<Trans context="danmu">Opacity</Trans>}
        value={Math.round(preferences.opacity * 100)}
        display={`${Math.round(preferences.opacity * 100)}%`}
        min={10}
        max={100}
        step={5}
        onChange={(value) => update({ opacity: value / 100 })}
      />

      <Section title={<Trans>Display area</Trans>}>
        <div className="grid grid-cols-4 gap-1.5">
          {DANMU_AREAS.map((area, index) => (
            <Chip
              key={area}
              active={preferences.area === area}
              onClick={() => update({ area })}
            >
              {i18n._(AREA_LABELS[index])}
            </Chip>
          ))}
        </div>
      </Section>

      <SliderRow
        label={<Trans>Font size</Trans>}
        value={Math.round(preferences.fontScale * 100)}
        display={`${Math.round(preferences.fontScale * 100)}%`}
        min={50}
        max={200}
        step={10}
        onChange={(value) => update({ fontScale: value / 100 })}
      />

      <SliderRow
        label={<Trans>Speed</Trans>}
        value={preferences.speed}
        display={i18n._(SPEED_LABELS[preferences.speed])}
        min={0}
        max={DANMU_SPEED_SECS.length - 1}
        step={1}
        onChange={(speed) => update({ speed })}
      />

      <div className="space-y-3">
        <SwitchRow
          label={<Trans>Merge repeated comments</Trans>}
          checked={preferences.merge}
          onChange={(merge) => update({ merge })}
        />
        <SwitchRow
          label={<Trans>Pause a comment on hover</Trans>}
          checked={preferences.hoverPause}
          onChange={(hoverPause) => update({ hoverPause })}
        />
        <SwitchRow
          label={<Trans>Show activity above the progress bar</Trans>}
          checked={preferences.heatmap}
          onChange={(heatmap) => update({ heatmap })}
        />
      </div>

      <Section title={<Trans>Blocklist</Trans>}>
        <div className="flex gap-1.5">
          <Chip
            active={blockKind === 'keyword'}
            onClick={() => setBlockKind('keyword')}
          >
            <Trans context="danmu blocklist">Word</Trans>
          </Chip>
          <Chip
            active={blockKind === 'user'}
            onClick={() => setBlockKind('user')}
          >
            <Trans context="danmu">User</Trans>
          </Chip>
        </div>
        <form
          className="flex gap-1.5"
          onSubmit={(event) => {
            event.preventDefault();
            addEntry();
          }}
        >
          <input
            value={entry}
            maxLength={BLOCK_ENTRY_MAX_LENGTH}
            onChange={(event) => setEntry(event.target.value)}
            placeholder={i18n._(BLOCK_PLACEHOLDER[blockKind])}
            aria-label={i18n._(BLOCK_PLACEHOLDER[blockKind])}
            className="h-8 min-w-0 flex-1 rounded-md border border-input bg-transparent px-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
          />
          <button
            type="submit"
            disabled={!canAdd}
            className="h-8 shrink-0 rounded-md bg-primary px-3 text-xs text-primary-foreground disabled:opacity-50"
          >
            <Trans context="danmu blocklist">Block</Trans>
          </button>
        </form>
        {blocked.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            <Trans>Nothing blocked.</Trans>
          </p>
        ) : (
          <ul className="flex max-h-28 flex-wrap gap-1.5 overflow-y-auto">
            {blocked.map(({ key, value }) => (
              <li
                key={`${key}:${value}`}
                className="flex items-center gap-1 rounded-md bg-muted px-2 py-0.5 text-xs"
              >
                {key === 'blockedUsers' && (
                  <span className="text-muted-foreground">@</span>
                )}
                <span className="max-w-40 truncate">{value}</span>
                <button
                  type="button"
                  aria-label={i18n._(msg`Unblock ${value}`)}
                  onClick={() => removeEntry(key, value)}
                  className="text-muted-foreground hover:text-foreground"
                >
                  <X className="size-3" />
                </button>
              </li>
            ))}
          </ul>
        )}
      </Section>
    </div>
  );
}
