import {
  createEngine,
  getEngine,
  listEngines,
  updateEngine,
} from '../functions/engines';

const fetchBackendMock = vi.hoisted(() => vi.fn());

vi.mock('@/server/createServerFn', async () => ({
  createServerFn: (await import('../createServerFn.desktop')).createServerFn,
}));
vi.mock('../api', async () => ({
  fetchBackend: fetchBackendMock,
  BackendApiError: (await import('@/lib/api-error')).BackendApiError,
}));

const choices = [
  undefined,
  null,
  '',
  '   ',
  '/opt/media tools/ffmpeg',
  'C:\\Media Tools\\ffmpeg.exe',
];

describe('Streamlink engine FFmpeg path serialization', () => {
  beforeEach(() => fetchBackendMock.mockReset());

  it.each([false, true])(
    'preserves path choices through reads and writes (JSON string: %s)',
    async (encoded) => {
      for (const ffmpegPath of choices) {
        const config =
          ffmpegPath === undefined
            ? { quality: 'best' }
            : { quality: 'best', ffmpeg_path: ffmpegPath };
        const row = {
          id: 'streamlink-1',
          name: 'Streamlink',
          engine_type: 'STREAMLINK',
          config: encoded ? JSON.stringify(config) : config,
        };
        fetchBackendMock.mockResolvedValueOnce(row);
        const loaded = await getEngine({ data: row.id });
        expect(JSON.parse(JSON.stringify(loaded.config))).toMatchObject(config);
        if (ffmpegPath === undefined)
          expect(loaded.config).not.toHaveProperty('ffmpeg_path');

        fetchBackendMock.mockResolvedValueOnce([row]);
        const listed = await listEngines();
        expect(listed[0].config).toEqual(loaded.config);

        fetchBackendMock.mockResolvedValueOnce(row);
        await createEngine({
          data: {
            name: row.name,
            engine_type: 'STREAMLINK',
            config: loaded.config,
          },
        });
        let [url, request] = fetchBackendMock.mock.calls.at(-1)!;
        expect(url).toBe('/engines');
        expect(request.method).toBe('POST');
        let sent = JSON.parse(request.body);
        expect(sent.config).toMatchObject(config);
        if (ffmpegPath === undefined)
          expect(sent.config).not.toHaveProperty('ffmpeg_path');

        fetchBackendMock.mockResolvedValueOnce(row);
        await updateEngine({
          data: {
            id: row.id,
            data: {
              engine_type: 'STREAMLINK',
              config: { ...loaded.config, quality: 'worst' },
            },
          },
        });
        [url, request] = fetchBackendMock.mock.calls.at(-1)!;
        expect(url).toBe('/engines/streamlink-1');
        expect(request.method).toBe('PATCH');
        sent = JSON.parse(request.body);
        expect(sent.config).toMatchObject({ ...config, quality: 'worst' });
        if (ffmpegPath === undefined)
          expect(sent.config).not.toHaveProperty('ffmpeg_path');
      }
    },
  );

  it('preserves an explicit reset without an engine type on PATCH', async () => {
    fetchBackendMock.mockResolvedValue({
      id: 'streamlink-1',
      name: 'Streamlink',
      engine_type: 'STREAMLINK',
      config: { ffmpeg_path: null },
    });
    await updateEngine({
      data: { id: 'streamlink-1', data: { config: { ffmpeg_path: null } } },
    });
    expect(JSON.parse(fetchBackendMock.mock.calls[0][1].body)).toEqual({
      config: { ffmpeg_path: null },
    });
  });

  it('rejects a malformed path before sending a create request', async () => {
    await expect(
      createEngine({
        data: {
          name: 'Streamlink',
          engine_type: 'STREAMLINK',
          config: { ffmpeg_path: 42 },
        },
      }),
    ).rejects.toThrow('ffmpeg_path');
    expect(fetchBackendMock).not.toHaveBeenCalled();
  });
});
