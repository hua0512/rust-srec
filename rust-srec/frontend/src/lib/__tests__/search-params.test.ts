import { describe, expect, it } from 'vitest';
import { z } from 'zod';

import { searchParamsValidator } from '../search-params';

const validate = searchParamsValidator({
  q: z.string().optional(),
  page: z.number().int().min(0).optional(),
  size: z.number().int().positive().optional(),
  priority: z.enum(['HIGH', 'LOW']).optional(),
  format: z
    .string()
    .optional()
    .transform((value) => value?.toUpperCase()),
});

describe('searchParamsValidator', () => {
  it('keeps every valid field', () => {
    expect(
      validate({ q: 'clip', page: 2, size: 24, priority: 'HIGH' }),
    ).toEqual({ q: 'clip', page: 2, size: 24, priority: 'HIGH' });
  });

  it('omits absent fields rather than storing undefined', () => {
    expect(Object.keys(validate({ q: 'clip' }))).toEqual(['q']);
  });

  it('applies transforms', () => {
    expect(validate({ format: 'video' })).toEqual({ format: 'VIDEO' });
  });

  // The whole point: one bad field used to throw and take the route's error
  // component with it, losing the parameters that were fine.
  it('drops only the fields that fail and keeps the rest', () => {
    expect(validate({ q: 'clip', page: 'abc', size: 24 })).toEqual({
      q: 'clip',
      size: 24,
    });
  });

  it.each([
    ['a non-numeric page', { page: 'abc' }],
    ['a negative page', { page: -5 }],
    ['a zero size', { size: 0 }],
    ['an unknown enum value', { priority: 'URGENT' }],
    ['a wrongly typed string', { q: ['a', 'b'] }],
  ])('never throws on %s', (_label, search) => {
    expect(() => validate(search)).not.toThrow();
    expect(validate(search)).toEqual({});
  });

  it('ignores unknown parameters', () => {
    expect(validate({ q: 'clip', utm_source: 'newsletter' })).toEqual({
      q: 'clip',
    });
  });

  it('retains a field default when the value is absent', () => {
    const withDefault = searchParamsValidator({
      size: z.number().int().positive().optional().default(24),
    });
    expect(withDefault({})).toEqual({ size: 24 });
    expect(withDefault({ size: 'nope' })).toEqual({ size: 24 });
  });
});
