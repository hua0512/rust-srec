import {
  CreateFilterRequestSchema,
  normalizeFilterConfigForType,
  UpdateFilterRequestSchema,
  type FilterType,
} from '../filter';

const timeWindow = {
  days_of_week: ['Monday'],
  start_time: '19:00',
  end_time: '23:00',
};

describe('filter timezone payloads', () => {
  it.each(['local', 'Europe/Madrid', 'UTC'])(
    'preserves %s through a TimeBased editor round trip',
    (timezone) => {
      const config = normalizeFilterConfigForType('TIME_BASED', {
        ...timeWindow,
        timezone,
      });
      const request = UpdateFilterRequestSchema.parse({
        filter_type: 'TIME_BASED',
        config,
      });

      expect(request.config).toEqual({
        ...timeWindow,
        start_time: '19:00:00',
        end_time: '23:00:00',
        timezone,
      });
    },
  );

  it('preserves the zone when normalizing legacy day names', () => {
    const config = normalizeFilterConfigForType('TIME_BASED', {
      days: ['Mon', 'Fri'],
      start_time: '22:00',
      end_time: '06:00',
      timezone: 'Asia/Shanghai',
    });

    expect(
      UpdateFilterRequestSchema.parse({ filter_type: 'TIME_BASED', config })
        .config,
    ).toEqual({
      days_of_week: ['Monday', 'Friday'],
      start_time: '22:00:00',
      end_time: '06:00:00',
      timezone: 'Asia/Shanghai',
    });
  });

  it.each<FilterType>(['TIME_BASED', 'CRON'])(
    '%s materializes missing/null zones as UTC without changing explicit zones',
    (filterType) => {
      const fields =
        filterType === 'TIME_BASED'
          ? timeWindow
          : { expression: '0 0 19 * * *' };

      for (const timezone of [undefined, null, 'local', 'Europe/Madrid']) {
        const config = normalizeFilterConfigForType(filterType, {
          ...fields,
          timezone,
        });
        const request = CreateFilterRequestSchema.parse({
          filter_type: filterType,
          config,
        });
        expect(request.config).toHaveProperty('timezone', timezone ?? 'UTC');
      }
    },
  );

  it.each<FilterType>(['TIME_BASED', 'CRON'])(
    '%s accepts explicit null and does not silently default a malformed zone',
    (filterType) => {
      const fields =
        filterType === 'TIME_BASED'
          ? timeWindow
          : { expression: '0 0 19 * * *' };

      expect(
        UpdateFilterRequestSchema.parse({
          filter_type: filterType,
          config: { ...fields, timezone: null },
        }).config,
      ).toHaveProperty('timezone', null);

      for (const timezone of [42, {}, ['UTC']]) {
        expect(
          UpdateFilterRequestSchema.safeParse({
            filter_type: filterType,
            config: normalizeFilterConfigForType(filterType, {
              ...fields,
              timezone,
            }),
          }).success,
        ).toBe(false);
      }
    },
  );
});
