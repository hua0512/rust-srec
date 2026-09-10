import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ComponentProps } from 'react';

import { createFilter, updateFilter } from '@/server/functions';
import { FilterDialog } from '../FilterDialog';

vi.mock('@/server/functions', () => ({
  createFilter: vi.fn(),
  updateFilter: vi.fn(),
}));

type Filter = NonNullable<ComponentProps<typeof FilterDialog>['filterToEdit']>;

const existingFilter: Filter = {
  id: 'filter-1',
  streamer_id: 'streamer-1',
  filter_type: 'TIME_BASED',
  config: {
    days_of_week: ['Monday'],
    start_time: '19:00:00',
    end_time: '23:00:00',
    timezone: 'local',
  },
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(createFilter).mockResolvedValue(existingFilter);
  vi.mocked(updateFilter).mockResolvedValue(existingFilter);
});

function renderDialog(filter?: Filter) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <I18nProvider i18n={setupI18n({ locale: 'en', messages: { en: {} } })}>
      <QueryClientProvider client={client}>
        <FilterDialog
          streamerId="streamer-1"
          open
          onOpenChange={vi.fn()}
          filterToEdit={filter}
        />
      </QueryClientProvider>
    </I18nProvider>,
  );
}

describe('filter dialog timezone submissions', () => {
  it.each(['local', 'Europe/Madrid'])(
    'keeps saved %s when only the recording hours change',
    async (timezone) => {
      renderDialog({
        ...existingFilter,
        config: { ...existingFilter.config, timezone },
      });
      fireEvent.change(await screen.findByLabelText('Start'), {
        target: { value: '20:00:00' },
      });
      fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));

      await waitFor(() => expect(updateFilter).toHaveBeenCalledTimes(1));
      expect(vi.mocked(updateFilter).mock.calls[0][0]).toEqual({
        data: {
          streamerId: 'streamer-1',
          filterId: 'filter-1',
          data: {
            filter_type: 'TIME_BASED',
            config: {
              ...existingFilter.config,
              start_time: '20:00:00',
              timezone,
            },
          },
        },
      });
    },
  );

  it.each(['Time Based', 'Cron'])(
    'creates %s filters with explicit UTC',
    async (label) => {
      renderDialog();
      fireEvent.click(screen.getByRole('radio', { name: new RegExp(label) }));
      fireEvent.click(screen.getByRole('button', { name: 'Create filter' }));

      await waitFor(() => expect(createFilter).toHaveBeenCalledTimes(1));
      expect(vi.mocked(createFilter).mock.calls[0][0].data.data).toMatchObject({
        filter_type: label === 'Cron' ? 'CRON' : 'TIME_BASED',
        config: { timezone: 'UTC' },
      });
    },
  );

  it('starts the UTC default when changing an existing filter type', async () => {
    renderDialog({
      ...existingFilter,
      filter_type: 'CRON',
      config: { expression: '0 0 19 * * *', timezone: 'Europe/Madrid' },
    });
    fireEvent.click(screen.getByRole('radio', { name: /Time Based/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));

    await waitFor(() => expect(updateFilter).toHaveBeenCalledTimes(1));
    expect(vi.mocked(updateFilter).mock.calls[0][0].data.data).toEqual({
      filter_type: 'TIME_BASED',
      config: {
        days_of_week: [],
        start_time: '00:00:00',
        end_time: '23:59:59',
        timezone: 'UTC',
      },
    });
  });
});
