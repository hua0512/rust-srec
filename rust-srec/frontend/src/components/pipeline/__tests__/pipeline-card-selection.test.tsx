import { fireEvent, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { PipelineSummaryCard } from '../jobs/pipeline-summary-card';
import { renderWithProviders } from '@/test/render';

vi.mock('@tanstack/react-router', async () => ({
  Link: (await import('@/test/router')).MockLink,
}));

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

describe('PipelineSummaryCard selection mode', () => {
  const onSelectChange = vi.fn();
  const onViewDetails = vi.fn();

  beforeEach(() => {
    onSelectChange.mockClear();
    onViewDetails.mockClear();
  });

  it('toggles selection with pointer and keyboard instead of navigating', () => {
    const { rerender } = renderWithProviders(
      <PipelineSummaryCard
        pipeline={pipeline}
        onViewDetails={onViewDetails}
        selectionMode
        isSelected={false}
        onSelectChange={onSelectChange}
      />,
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
});
