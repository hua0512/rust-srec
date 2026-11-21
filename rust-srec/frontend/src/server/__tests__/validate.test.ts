import { z } from 'zod';
import { parseInput } from '../validate';

describe('parseInput', () => {
  it('reports the failing field as a plain Error', () => {
    const schema = z.object({ settings: z.string() });
    let thrown: unknown;
    try {
      parseInput(schema, { settings: {} });
    } catch (error) {
      thrown = error;
    }
    expect(thrown).toBeInstanceOf(Error);
    expect(thrown).not.toBeInstanceOf(z.ZodError);
    expect((thrown as Error).message).toBe(
      'settings: Invalid input: expected string, received object',
    );
  });

  it('reports a top-level failure without a field prefix', () => {
    expect(() => parseInput(z.string().min(1), '')).toThrow(
      'Too small: expected string to have >=1 characters',
    );
  });

  it('summarizes at most three issues', () => {
    const schema = z.object({
      a: z.string(),
      b: z.string(),
      c: z.string(),
      d: z.string(),
    });
    expect(() => parseInput(schema, {})).toThrow(/\(and 1 more\)$/);
  });
});
