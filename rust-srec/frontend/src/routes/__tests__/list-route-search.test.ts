import { describe, expect, it } from 'vitest';

import { Route as SessionsRoute } from '../_authed/_dashboard/sessions/index';
import { Route as EventsRoute } from '../_authed/_dashboard/notifications/events';
import { Route as TemplatesRoute } from '../_authed/_dashboard/config/templates/index';

/**
 * A stale, truncated or hand-edited address used to replace these pages with
 * the router's error component. Every value the page cannot read has to be
 * dropped on its own, leaving the rest of the address working.
 */
const routes = [
  ['sessions', SessionsRoute],
  ['notification events', EventsRoute],
  ['templates', TemplatesRoute],
] as const;

function validate(route: (typeof routes)[number][1], search: object) {
  const validateSearch = route.options.validateSearch as (
    search: Record<string, unknown>,
  ) => Record<string, unknown>;
  return validateSearch(search as Record<string, unknown>);
}

describe.each(routes)('%s search params', (_label, route) => {
  it('accepts an empty address', () => {
    expect(() => validate(route, {})).not.toThrow();
    expect(validate(route, {})).toEqual({});
  });

  it('ignores unreadable values instead of failing the page', () => {
    const search = {
      page: 'not-a-number',
      status: 'nonsense',
      priority: { nested: true },
      q: ['a', 'b'],
      search: ['a', 'b'],
      type: 42,
    };

    expect(() => validate(route, search)).not.toThrow();
    expect(validate(route, search)).toEqual({});
  });
});

describe('sessions search params', () => {
  it('keeps the readable half of a partly broken address', () => {
    expect(validate(SessionsRoute, { search: 'clip', page: 'abc' })).toEqual({
      search: 'clip',
    });
  });
});

describe('notification events search params', () => {
  it('keeps the readable half of a partly broken address', () => {
    expect(validate(EventsRoute, { q: 'streamer-1', page: 0 })).toEqual({
      q: 'streamer-1',
    });
  });
});

describe('templates search params', () => {
  it('keeps a valid search term', () => {
    expect(validate(TemplatesRoute, { q: 'archive' })).toEqual({
      q: 'archive',
    });
  });
});
