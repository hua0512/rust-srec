import { setupI18n } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { SplitReasonDetails } from '../split-reason';

// Descriptors built here carry the same generated ids as the ones inside
// `split-reason.ts`, so a catalog keyed by them proves the diff-table labels
// are resolved through `i18n` rather than hardcoded English.
const codec = msg`Codec`;
const sampleRate = msg`Sample Rate`;
const from = msg({ message: 'From', context: 'Comparison table column' });
const to = msg({ message: 'To', context: 'Comparison table column' });

const i18n = setupI18n({
  locale: 'xx',
  messages: {
    xx: {
      [String(codec.id)]: 'CODEC',
      [String(sampleRate.id)]: 'SAMPLE RATE',
      [String(from.id)]: 'BEFORE',
      [String(to.id)]: 'AFTER',
    },
  },
});

describe('SplitReasonDetails', () => {
  it('translates the video codec diff table', () => {
    render(
      <SplitReasonDetails
        i18n={i18n}
        code="video_codec_change"
        details={{ from: { codec: 'h264' }, to: { codec: 'hevc' } }}
      />,
    );

    expect(screen.getByText('BEFORE')).toBeInTheDocument();
    expect(screen.getByText('AFTER')).toBeInTheDocument();
    expect(screen.getByText('CODEC')).toBeInTheDocument();
    expect(screen.getByText('h264')).toBeInTheDocument();
    expect(screen.getByText('hevc')).toBeInTheDocument();
  });

  it('translates the audio codec diff table', () => {
    render(
      <SplitReasonDetails
        i18n={i18n}
        code="audio_codec_change"
        details={{
          from: { codec: 'aac', sample_rate: 44100 },
          to: { codec: 'aac', sample_rate: 48000 },
        }}
      />,
    );

    expect(screen.getByText('SAMPLE RATE')).toBeInTheDocument();
    expect(screen.getByText('44.1kHz')).toBeInTheDocument();
    expect(screen.getByText('48kHz')).toBeInTheDocument();
  });

  it('keeps the column headings separate from the unrelated "To" message', () => {
    expect(to.id).not.toBe(msg`To`.id);
  });
});
