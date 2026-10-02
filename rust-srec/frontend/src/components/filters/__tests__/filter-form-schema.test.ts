import { setupI18n } from '@lingui/core';

import { getFilterFormSchema } from '../FilterDialog';

const schema = getFilterFormSchema(
  setupI18n({ locale: 'en', messages: { en: {} } }),
);

function timeBased(config: Record<string, unknown>) {
  return {
    filter_type: 'TIME_BASED',
    config: {
      days_of_week: ['Monday'],
      start_time: '21:00:00',
      end_time: '06:00:00',
      timezone: 'UTC',
      ...config,
    },
  };
}

function issuePaths(input: unknown) {
  const result = schema.safeParse(input);
  return result.success ? [] : result.error.issues.map((i) => i.path.join('.'));
}

describe('filter form schema', () => {
  it('accepts an overnight window', () => {
    expect(issuePaths(timeBased({}))).toEqual([]);
  });

  it('requires at least one day', () => {
    expect(issuePaths(timeBased({ days_of_week: [] }))).toEqual([
      'config.days_of_week',
    ]);
  });

  it('rejects equal start and end after normalizing seconds', () => {
    expect(
      issuePaths(timeBased({ start_time: '21:00', end_time: '21:00:00' })),
    ).toEqual(['config.end_time']);
  });

  it('leaves other filter types alone', () => {
    expect(
      issuePaths({
        filter_type: 'KEYWORD',
        config: { include: [], exclude: [] },
      }),
    ).toEqual([]);
  });
});
