import { setupI18n } from '@lingui/core';
import { I18nProvider } from '@lingui/react';
import { fireEvent, render, screen } from '@testing-library/react';
import type { z } from 'zod';

import { FilterSchema } from '@/api/schemas';
import { FilterCard } from '../FilterCard';

describe.each(['TIME_BASED', 'CRON'] as const)(
  '%s card timezone',
  (filterType) => {
    it.each([
      { timezone: undefined, label: 'UTC' },
      { timezone: null, label: 'UTC' },
      { timezone: 'UTC', label: 'UTC' },
      { timezone: 'local', label: 'Server local' },
      { timezone: 'America/New_York', label: 'America/New_York' },
    ])(
      'shows $label for $timezone without changing edit data',
      ({ timezone, label }) => {
        const config = {
          ...(filterType === 'TIME_BASED'
            ? {
                days_of_week: ['Monday'],
                start_time: '19:15:00',
                end_time: '02:30:00',
              }
            : { expression: '0 15 19 * * MON' }),
          ...(timezone === undefined ? {} : { timezone }),
        };
        const filter: z.infer<typeof FilterSchema> = {
          id: 'filter-1',
          streamer_id: 'streamer-1',
          filter_type: filterType,
          config,
        };
        const onEdit = vi.fn();
        const before = structuredClone(filter);

        render(
          <I18nProvider
            i18n={setupI18n({ locale: 'en', messages: { en: {} } })}
          >
            <FilterCard filter={filter} onEdit={onEdit} onDelete={vi.fn()} />
          </I18nProvider>,
        );

        expect(screen.getByText(`Timezone: ${label}`)).toBeVisible();
        if (filterType === 'TIME_BASED') {
          expect(screen.getByText('19:15', { exact: false })).toHaveTextContent(
            '19:15–02:30',
          );
        } else {
          expect(screen.getByText('0 15 19 * * MON')).toBeVisible();
        }

        fireEvent.click(screen.getByRole('button', { name: 'Edit filter' }));
        expect(onEdit).toHaveBeenCalledWith(filter);
        expect(filter).toEqual(before);
      },
    );
  },
);
