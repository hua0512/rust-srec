import { getEngine, createEngine, updateEngine } from '../functions/engines';

const fetchBackendMock = vi.hoisted(() => vi.fn());

vi.mock('@/server/createServerFn', async () => ({
  createServerFn: (await import('../createServerFn.desktop')).createServerFn,
}));
vi.mock('../api', async () => ({
  fetchBackend: fetchBackendMock,
  BackendApiError: (await import('@/lib/api-error')).BackendApiError,
}));

beforeEach(() => fetchBackendMock.mockReset());

it.each(['FFMPEG', 'STREAMLINK'] as const)(
  'preserves %s lossless cutting through engine reads and writes',
  async (engine_type) => {
    for (const enabled of [undefined, true, false]) {
      const config =
        enabled === undefined ? {} : { enable_lossless_cutting: enabled };
      const row = {
        id: 'engine',
        name: 'Recorder',
        engine_type,
        config: JSON.stringify(config),
      };
      fetchBackendMock.mockResolvedValue(row);
      const loaded = await getEngine({ data: row.id });
      expect(loaded.config).toHaveProperty(
        'enable_lossless_cutting',
        enabled ?? false,
      );
      await createEngine({
        data: { name: row.name, engine_type, config: loaded.config },
      });
      expect(
        JSON.parse(fetchBackendMock.mock.calls.at(-1)![1].body).config
          .enable_lossless_cutting,
      ).toBe(enabled ?? false);
      await updateEngine({
        data: { id: row.id, data: { engine_type, config: loaded.config } },
      });
      expect(
        JSON.parse(fetchBackendMock.mock.calls.at(-1)![1].body).config
          .enable_lossless_cutting,
      ).toBe(enabled ?? false);
    }
  },
);
