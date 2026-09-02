import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen } from '@testing-library/react';
import type { ReactNode } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { PipelineSummaryCard } from '../jobs/pipeline-summary-card';
import { OutputCard } from '../outputs/output-card';

vi.mock('@tanstack/react-router', () => ({
  Link: ({ children }: { children: ReactNode }) => <a>{children}</a>,
}));

const i18n = setupI18n({ locale: 'en', messages: { en: {} } });

// Both cards call `useLingui` for their labels, so every render needs a provider.
function Wrapper({ children }: { children: ReactNode }) {
  return <I18nProvider i18n={i18n}>{children}</I18nProvider>;
}

const pipeline = {
  id: 'dag-1',
  name: 'Pipeline One',
  status: 'COMPLETED' as const,
  streamer_id: 'streamer-1',
  streamer_name: 'Streamer One',
  session_id: 'session-1',
  total_steps: 3,
  completed_steps: 3,
  failed_steps: 0,
  progress_percent: 100,
  created_at: 1767225600000,
  updated_at: 1767225600000,
};

const output = {
  id: 'output-1',
  session_id: 'session-1',
  streamer_id: 'streamer-1',
  file_path: '/recordings/streamer-one/clip.mp4',
  file_size_bytes: 1024,
  // `format` carries `media_outputs.file_type`, never a container extension.
  format: 'VIDEO',
  created_at: '2026-01-01T00:00:00Z',
  uploads: [],
};

describe('PipelineSummaryCard selection mode', () => {
  const onSelectChange = vi.fn();
  const onViewDetails = vi.fn();

  beforeEach(() => {
    onSelectChange.mockClear();
    onViewDetails.mockClear();
  });

  it('toggles selection with pointer and keyboard instead of navigating', () => {
    const { rerender } = render(
      <PipelineSummaryCard
        pipeline={pipeline}
        onViewDetails={onViewDetails}
        selectionMode
        isSelected={false}
        onSelectChange={onSelectChange}
      />,
      { wrapper: Wrapper },
    );

    const card = screen.getByRole('checkbox');
    expect(card).toHaveAttribute('aria-checked', 'false');

    fireEvent.click(card);
    expect(onSelectChange).toHaveBeenCalledWith('dag-1', true);
    // Selection mode must not navigate to the execution detail page.
    expect(onViewDetails).not.toHaveBeenCalled();

    rerender(
      <PipelineSummaryCard
        pipeline={pipeline}
        onViewDetails={onViewDetails}
        selectionMode
        isSelected
        onSelectChange={onSelectChange}
      />,
    );

    const selectedCard = screen.getByRole('checkbox');
    expect(selectedCard).toHaveAttribute('aria-checked', 'true');
    fireEvent.keyDown(selectedCard, { key: 'Enter' });
    expect(onSelectChange).toHaveBeenLastCalledWith('dag-1', false);
  });

  it('navigates on click when selection mode is off', () => {
    const { container } = render(
      <PipelineSummaryCard
        pipeline={pipeline}
        onViewDetails={onViewDetails}
        onSelectChange={onSelectChange}
      />,
      { wrapper: Wrapper },
    );

    expect(screen.queryByRole('checkbox')).toBeNull();
    fireEvent.click(container.querySelector('[data-slot="card"]')!);
    expect(onViewDetails).toHaveBeenCalledWith('dag-1');
    expect(onSelectChange).not.toHaveBeenCalled();
  });
});

describe('OutputCard selection mode', () => {
  const onSelectChange = vi.fn();

  beforeEach(() => {
    onSelectChange.mockClear();
  });

  it('toggles selection with pointer and keyboard input', () => {
    const { rerender } = render(
      <OutputCard
        output={output}
        selectionMode
        isSelected={false}
        onSelectChange={onSelectChange}
      />,
      { wrapper: Wrapper },
    );

    const card = screen.getByRole('checkbox');
    expect(card).toHaveAttribute('aria-checked', 'false');

    fireEvent.click(card);
    expect(onSelectChange).toHaveBeenCalledWith('output-1', true);

    rerender(
      <OutputCard
        output={output}
        selectionMode
        isSelected
        onSelectChange={onSelectChange}
      />,
    );

    fireEvent.keyDown(screen.getByRole('checkbox'), { key: ' ' });
    expect(onSelectChange).toHaveBeenLastCalledWith('output-1', false);
  });

  it('exposes no checkbox role outside selection mode', () => {
    render(<OutputCard output={output} />, { wrapper: Wrapper });
    expect(screen.queryByRole('checkbox')).toBeNull();
  });
});
