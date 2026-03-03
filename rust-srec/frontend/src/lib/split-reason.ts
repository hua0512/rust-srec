import type { I18n } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import type React from 'react';
import { createElement as h } from 'react';

function isNonEmptyString(value: unknown): value is string {
  return typeof value === 'string' && value.trim().length > 0;
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function formatSignature(value: unknown): string | null {
  if (typeof value !== 'number' || !Number.isFinite(value)) return null;
  const unsigned = value >>> 0;
  return `0x${unsigned.toString(16).padStart(8, '0')}`;
}

function formatWxH(width: unknown, height: unknown): string | null {
  if (typeof width !== 'number' || typeof height !== 'number') return null;
  if (!Number.isFinite(width) || !Number.isFinite(height)) return null;
  return `${width}×${height}`;
}

function formatHz(value: unknown): string | null {
  if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0)
    return null;
  if (value >= 1000) {
    const khz = value / 1000;
    const pretty = Number.isInteger(khz) ? khz.toFixed(0) : khz.toFixed(1);
    return `${pretty}kHz`;
  }
  return `${value}Hz`;
}

function formatResolutionSide(details: unknown): string | null {
  if (!isObject(details)) return null;
  return formatWxH(details.width, details.height);
}

function formatSplitReasonFromCode(i18n: I18n, code: string): string | null {
  switch (code) {
    case 'size_limit':
      return i18n._(msg`Size limit`);
    case 'duration_limit':
      return i18n._(msg`Duration limit`);
    case 'header_received':
      return i18n._(msg`Header received`);
    case 'discontinuity':
      return i18n._(msg`Discontinuity`);
    case 'stream_structure_change':
      return i18n._(msg`Stream structure change`);
    case 'resolution_change':
      return i18n._(msg`Resolution change`);
    case 'video_codec_change':
      return i18n._(msg`Video codec change`);
    case 'audio_codec_change':
      return i18n._(msg`Audio codec change`);
    default:
      return code;
  }
}

export function formatSplitReason(i18n: I18n, reason: unknown): string | null {
  if (isObject(reason)) {
    const code = reason.code;
    if (isNonEmptyString(code)) {
      return formatSplitReasonFromCode(i18n, code.trim());
    }
    return null;
  }
  return null;
}

type Row = { label: string; from: string; to: string };

function monoCell(text: string): React.ReactElement {
  return h(
    'span',
    { className: 'font-mono text-[11px] text-foreground' },
    text,
  );
}

function labelCell(text: string): React.ReactElement {
  return h(
    'span',
    { className: 'text-[11px] text-muted-foreground pr-3 whitespace-nowrap' },
    text,
  );
}

function ComparisonTable({ rows }: { rows: Row[] }): React.ReactElement {
  return h(
    'table',
    { className: 'mt-1 w-full border-collapse' },
    h(
      'thead',
      null,
      h(
        'tr',
        null,
        h('th', { className: 'w-[30%]' }),
        h(
          'th',
          {
            className:
              'text-[10px] font-medium text-muted-foreground pb-1 pr-3 text-left',
          },
          'From',
        ),
        h(
          'th',
          {
            className:
              'text-[10px] font-medium text-muted-foreground pb-1 text-left',
          },
          'To',
        ),
      ),
    ),
    h(
      'tbody',
      null,
      ...rows.map((row, i) =>
        h(
          'tr',
          { key: i, className: 'border-t border-border/30' },
          h('td', { className: 'py-0.5' }, labelCell(row.label)),
          h('td', { className: 'py-0.5 pr-3' }, monoCell(row.from)),
          h('td', { className: 'py-0.5' }, monoCell(row.to)),
        ),
      ),
    ),
  );
}

function videoCodecRows(details: Record<string, unknown>): Row[] {
  const from = isObject(details.from) ? details.from : {};
  const to = isObject(details.to) ? details.to : {};
  const rows: Row[] = [];

  rows.push({
    label: 'Codec',
    from: isNonEmptyString(from.codec) ? from.codec : '—',
    to: isNonEmptyString(to.codec) ? to.codec : '—',
  });

  const fProfile =
    typeof from.profile === 'number' && Number.isFinite(from.profile)
      ? String(from.profile)
      : null;
  const tProfile =
    typeof to.profile === 'number' && Number.isFinite(to.profile)
      ? String(to.profile)
      : null;
  if (fProfile || tProfile) {
    rows.push({ label: 'Profile', from: fProfile ?? '—', to: tProfile ?? '—' });
  }

  const fLevel =
    typeof from.level === 'number' && Number.isFinite(from.level)
      ? String(from.level)
      : null;
  const tLevel =
    typeof to.level === 'number' && Number.isFinite(to.level)
      ? String(to.level)
      : null;
  if (fLevel || tLevel) {
    rows.push({ label: 'Level', from: fLevel ?? '—', to: tLevel ?? '—' });
  }

  const fRes = formatWxH(from.width, from.height);
  const tRes = formatWxH(to.width, to.height);
  if (fRes || tRes) {
    rows.push({ label: 'Resolution', from: fRes ?? '—', to: tRes ?? '—' });
  }

  const fSig = formatSignature(from.signature);
  const tSig = formatSignature(to.signature);
  if (fSig || tSig) {
    rows.push({ label: 'Signature', from: fSig ?? '—', to: tSig ?? '—' });
  }

  return rows;
}

function audioCodecRows(details: Record<string, unknown>): Row[] {
  const from = isObject(details.from) ? details.from : {};
  const to = isObject(details.to) ? details.to : {};
  const rows: Row[] = [];

  rows.push({
    label: 'Codec',
    from: isNonEmptyString(from.codec) ? from.codec : '—',
    to: isNonEmptyString(to.codec) ? to.codec : '—',
  });

  const fHz = formatHz(from.sample_rate);
  const tHz = formatHz(to.sample_rate);
  if (fHz || tHz) {
    rows.push({ label: 'Sample Rate', from: fHz ?? '—', to: tHz ?? '—' });
  }

  const fCh =
    typeof from.channels === 'number' &&
    Number.isFinite(from.channels) &&
    from.channels > 0
      ? `${from.channels}ch`
      : null;
  const tCh =
    typeof to.channels === 'number' &&
    Number.isFinite(to.channels) &&
    to.channels > 0
      ? `${to.channels}ch`
      : null;
  if (fCh || tCh) {
    rows.push({ label: 'Channels', from: fCh ?? '—', to: tCh ?? '—' });
  }

  const fSig = formatSignature(from.signature);
  const tSig = formatSignature(to.signature);
  if (fSig || tSig) {
    rows.push({ label: 'Signature', from: fSig ?? '—', to: tSig ?? '—' });
  }

  return rows;
}

export function SplitReasonDetails({
  code,
  details,
}: {
  code: string;
  details: unknown;
}): React.ReactElement | null {
  switch (code) {
    case 'video_codec_change': {
      if (!isObject(details)) return null;
      const rows = videoCodecRows(details);
      if (rows.length === 0) return null;
      return h(
        'div',
        {
          className:
            'mt-2 rounded-md border border-border/60 bg-muted/30 px-2 pb-1.5',
        },
        h(ComparisonTable, { rows }),
      );
    }
    case 'audio_codec_change': {
      if (!isObject(details)) return null;
      const rows = audioCodecRows(details);
      if (rows.length === 0) return null;
      return h(
        'div',
        {
          className:
            'mt-2 rounded-md border border-border/60 bg-muted/30 px-2 pb-1.5',
        },
        h(ComparisonTable, { rows }),
      );
    }
    case 'resolution_change': {
      if (!isObject(details)) return null;
      const from = formatResolutionSide(details.from);
      const to = formatResolutionSide(details.to);
      if (!from && !to) return null;
      return h(
        'div',
        {
          className:
            'mt-2 flex items-center gap-2 rounded-md border border-border/60 bg-muted/30 px-3 py-1.5 font-mono text-[11px]',
        },
        h('span', { className: 'text-foreground' }, from ?? '—'),
        h('span', { className: 'text-muted-foreground' }, '→'),
        h('span', { className: 'text-foreground' }, to ?? '—'),
      );
    }
    case 'stream_structure_change': {
      if (!isObject(details)) return null;
      const desc = details.description;
      if (!isNonEmptyString(desc)) return null;
      return h(
        'div',
        {
          className:
            'mt-2 rounded-md border border-border/60 bg-muted/30 px-3 py-1.5 text-[11px] text-muted-foreground break-words',
        },
        desc,
      );
    }
    default:
      return null;
  }
}
