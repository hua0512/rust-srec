import { type ComponentPropsWithoutRef } from 'react';
import { UseFormReturn } from 'react-hook-form';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Trans } from '@lingui/react/macro';
import {
  EditableCombobox,
  type EditableComboboxOption,
} from '@/components/ui/editable-combobox';

interface DouyuQualityComboboxProps extends Omit<
  ComponentPropsWithoutRef<'div'>,
  'onChange'
> {
  fieldName: string;
  form: UseFormReturn<any>;
  onChange: (value: number) => void;
  value: unknown;
}

const AUDIO_ONLY_QUALITY = 'audio_only';

const DOUYU_QUALITY_OPTIONS = [
  {
    aliases: ['0', 'od', 'bd', 'original', 'blu_ray', 'blue_ray'],
    label: msg`Original / Blu-ray`,
    note: msg`Default Douyu quality`,
    rate: 0,
  },
  {
    aliases: ['3', 'uhd', 'ultra_hd'],
    label: msg`Ultra HD`,
    note: msg`Douyu UHD rate`,
    rate: 3,
  },
  {
    aliases: ['2', 'hd'],
    label: msg`HD`,
    note: msg`Douyu HD rate`,
    rate: 2,
  },
  {
    aliases: ['1', 'sd', 'ld', 'low'],
    label: msg`SD / Low`,
    note: msg`Douyu SD or low rate`,
    rate: 1,
  },
];

function normalizeQualityInput(value: string) {
  return value
    .trim()
    .toLowerCase()
    .replace(/[\s-]+/g, '_');
}

function getDouyuRate(value: unknown) {
  if (typeof value === 'number' && Number.isFinite(value)) {
    return value;
  }

  if (typeof value === 'string') {
    const parsed = Number.parseInt(value, 10);
    return Number.isFinite(parsed) ? parsed : 0;
  }

  return 0;
}

function parseQualityInput(value: string) {
  const normalized = normalizeQualityInput(value);
  if (!normalized) return null;

  if (['audio', 'audio_only', 'aac'].includes(normalized)) {
    return { type: AUDIO_ONLY_QUALITY } as const;
  }

  const option = DOUYU_QUALITY_OPTIONS.find((quality) =>
    quality.aliases.includes(normalized),
  );
  if (option) {
    return { type: 'rate', rate: option.rate } as const;
  }

  if (/^-?\d+$/.test(normalized)) {
    return { type: 'rate', rate: Number.parseInt(normalized, 10) } as const;
  }

  return null;
}

export function DouyuQualityCombobox({
  fieldName,
  form,
  onChange,
  value,
  ...comboboxProps
}: DouyuQualityComboboxProps) {
  const { i18n } = useLingui();
  const onlyAudioPath = `${fieldName}.only_audio`;
  const onlyAudio = !!form.watch(onlyAudioPath);
  const rate = getDouyuRate(value);
  const selectedValue = onlyAudio ? AUDIO_ONLY_QUALITY : String(rate);

  const selectedOption = DOUYU_QUALITY_OPTIONS.find(
    (quality) => quality.rate === rate,
  );
  const displayValue = onlyAudio
    ? i18n._(msg`Audio only`)
    : selectedOption
      ? `${i18n._(selectedOption.label)} (${selectedOption.rate})`
      : `${i18n._(msg`Custom rate`)} (${rate})`;

  const applyRate = (nextRate: number) => {
    onChange(nextRate);
    form.setValue(onlyAudioPath, false, { shouldDirty: true });
  };

  const applyAudioOnly = () => {
    onChange(0);
    form.setValue(onlyAudioPath, true, { shouldDirty: true });
  };

  const options: EditableComboboxOption[] = [
    {
      badge: 'AAC',
      description: <Trans>Request AAC audio without a video track.</Trans>,
      inputValue: i18n._(msg`Audio only`),
      label: <Trans>Audio only</Trans>,
      searchValue: 'audio only aac',
      value: AUDIO_ONLY_QUALITY,
    },
    ...DOUYU_QUALITY_OPTIONS.map((quality) => {
      const label = i18n._(quality.label);

      return {
        badge: quality.rate,
        description: i18n._(quality.note),
        inputValue: `${label} (${quality.rate})`,
        label,
        searchValue: `${label} ${quality.aliases.join(' ')}`,
        value: String(quality.rate),
      };
    }),
  ];

  const handleInputChange = (nextValue: string) => {
    const parsed = parseQualityInput(nextValue);
    if (!parsed) return;

    if (parsed.type === AUDIO_ONLY_QUALITY) {
      applyAudioOnly();
    } else {
      applyRate(parsed.rate);
    }
  };

  const handleInputBlur = (nextValue: string) => {
    const parsed = parseQualityInput(nextValue);
    if (!parsed && !nextValue.trim()) {
      applyRate(0);
    }
  };

  const handleOptionSelect = (option: EditableComboboxOption) => {
    if (option.value === AUDIO_ONLY_QUALITY) {
      applyAudioOnly();
    } else {
      applyRate(Number.parseInt(option.value, 10));
    }
  };

  return (
    <EditableCombobox
      {...comboboxProps}
      buttonLabel={i18n._(msg`Open quality options`)}
      displayValue={displayValue}
      onInputBlur={handleInputBlur}
      onInputChange={handleInputChange}
      onOptionSelect={handleOptionSelect}
      options={options}
      placeholder={i18n._(msg`Select or type a rate`)}
      selectedValue={selectedValue}
    />
  );
}
