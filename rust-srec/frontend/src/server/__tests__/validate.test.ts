import { z } from 'zod';
import { parseInput } from '../validate';
import { updateLoggingFilter } from '../functions/logging';

const fetchBackendMock = vi.hoisted(() => vi.fn());

vi.mock('@/server/createServerFn', async () => ({
  createServerFn: (await import('../createServerFn.desktop')).createServerFn,
}));

vi.mock('../api', async () => ({
  fetchBackend: fetchBackendMock,
  BackendApiError: (await import('@/lib/api-error')).BackendApiError,
}));

describe('parseInput', () => {
  it('returns the parsed value on success', () => {
    const schema = z.object({ id: z.string().min(1) });
    expect(parseInput(schema, { id: 'abc' })).toEqual({ id: 'abc' });
  });

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

describe('server function validator failures', () => {
  it('surfaces a readable message instead of a serialized ZodError', async () => {
    fetchBackendMock.mockResolvedValue(undefined);
    const failure = updateLoggingFilter({ data: { filter: '' } }).catch(
      (error: unknown) => error,
    );
    await expect(failure).resolves.toBeInstanceOf(Error);
    await expect(failure).resolves.not.toBeInstanceOf(z.ZodError);
    expect(((await failure) as Error).message).toContain('filter: ');
    expect(fetchBackendMock).not.toHaveBeenCalled();
  });
});
