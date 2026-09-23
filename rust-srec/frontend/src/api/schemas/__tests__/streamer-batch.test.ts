import { describe, expect, it } from 'vitest';

import { BatchStreamerRequestSchema } from '../streamer';

describe('BatchStreamerRequestSchema', () => {
  it.each([
    { type: 'set_enabled', enabled: true },
    { type: 'set_template', template_id: 'template-1' },
    { type: 'set_template', template_id: null },
    { type: 'set_priority', priority: 'HIGH' },
    { type: 'delete' },
  ])('accepts the $type action', (action) => {
    expect(
      BatchStreamerRequestSchema.safeParse({
        ids: ['streamer-1', 'streamer-2'],
        action,
      }).success,
    ).toBe(true);
  });

  it('rejects invalid ID collections', () => {
    expect(
      BatchStreamerRequestSchema.safeParse({
        ids: [],
        action: { type: 'delete' },
      }).success,
    ).toBe(false);
    expect(
      BatchStreamerRequestSchema.safeParse({
        ids: ['streamer-1', 'streamer-1'],
        action: { type: 'delete' },
      }).success,
    ).toBe(false);
    expect(
      BatchStreamerRequestSchema.safeParse({
        ids: Array.from({ length: 101 }, (_, index) => `streamer-${index}`),
        action: { type: 'delete' },
      }).success,
    ).toBe(false);
  });
});
