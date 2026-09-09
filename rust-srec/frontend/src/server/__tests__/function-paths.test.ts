import { revokeApiKey } from '../functions/apiKeys';
import { getTemplate, updateGlobalConfig } from '../functions/config';
import { getTemplateCredentialSource } from '../functions/credentials';
import { getEngine } from '../functions/engines';
import { createFilter, deleteFilter, updateFilter } from '../functions/filters';
import { getJobPreset } from '../functions/job';
import { listLogFiles } from '../functions/logging';
import { createChannel, getChannel } from '../functions/notifications';
import {
  createPipelinePreset,
  deletePipelineOutput,
  getPipelineJob,
  listPipelinePresets,
} from '../functions/pipeline';
import { getSession } from '../functions/sessions';
import {
  getStreamer,
  updateStreamer,
  updateStreamerPriority,
} from '../functions/streamers';

const fetchBackendMock = vi.hoisted(() => vi.fn());

// The server functions are exercised through the desktop shim, which runs the
// validator and handler in-process instead of over the TanStack Start RPC.
vi.mock('@/server/createServerFn', async () => ({
  createServerFn: (await import('../createServerFn.desktop')).createServerFn,
}));

vi.mock('../api', async () => ({
  fetchBackend: fetchBackendMock,
  BackendApiError: (await import('@/lib/api-error')).BackendApiError,
}));

beforeEach(() => {
  fetchBackendMock.mockReset();
  // Handlers parse the response; the tests only care about the request, so let
  // that parse fail and swallow it in `requestedPath`.
  fetchBackendMock.mockResolvedValue(undefined);
});

async function requestedPath(invoke: () => Promise<unknown>): Promise<string> {
  await invoke().catch(() => undefined);
  expect(fetchBackendMock).toHaveBeenCalledTimes(1);
  return fetchBackendMock.mock.calls[0][0] as string;
}

async function expectNoRequest(invoke: () => Promise<unknown>): Promise<void> {
  await expect(invoke()).rejects.toThrow();
  expect(fetchBackendMock).not.toHaveBeenCalled();
}

async function requestedBody(invoke: () => Promise<unknown>): Promise<string> {
  await invoke().catch(() => undefined);
  expect(fetchBackendMock).toHaveBeenCalledTimes(1);
  return (fetchBackendMock.mock.calls[0][1] as RequestInit).body as string;
}

const ID = '0f6b2f7e-6c2f-4c1a-9e3f-2f0a1b8c7d55';

/** Every required global setting, with the two boolean toggles deliberately left out. */
function globalConfigWithoutToggles() {
  return {
    output_folder: '/records',
    output_filename_template: '{streamer}',
    output_file_format: 'flv',
    min_segment_size_bytes: 1,
    max_download_duration_secs: 1,
    max_part_size_bytes: 1,
    record_danmu: true,
    max_concurrent_downloads: 1,
    max_concurrent_uploads: 1,
    streamer_check_delay_ms: 1000,
    offline_check_delay_ms: 1000,
    offline_check_count: 1,
    default_download_engine: 'ffmpeg',
    max_concurrent_cpu_jobs: 1,
    max_concurrent_io_jobs: 1,
    job_history_retention_days: 30,
    notification_event_log_retention_days: 30,
    log_filter_directive: 'info',
    pipeline_cpu_job_timeout_secs: 1,
    pipeline_io_job_timeout_secs: 1,
    pipeline_execute_timeout_secs: 1,
    queue_freshness_threshold_ms: 0,
    gpu_health_probe_interval_secs: 1,
  };
}

describe('server function request paths', () => {
  it.each([
    ['getStreamer', () => getStreamer({ data: ID }), `/streamers/${ID}`],
    [
      'updateStreamer',
      () => updateStreamer({ data: { id: ID, data: { enabled: true } } }),
      `/streamers/${ID}`,
    ],
    [
      'deleteFilter',
      () => deleteFilter({ data: { streamerId: ID, filterId: 'f1' } }),
      `/streamers/${ID}/filters/f1`,
    ],
    [
      'getChannel',
      () => getChannel({ data: ID }),
      `/notifications/channels/${ID}`,
    ],
    ['getTemplate', () => getTemplate({ data: ID }), `/templates/${ID}`],
    ['getSession', () => getSession({ data: ID }), `/sessions/${ID}`],
    ['getJobPreset', () => getJobPreset({ data: ID }), `/job/presets/${ID}`],
    ['getEngine', () => getEngine({ data: ID }), `/engines/${ID}`],
    ['revokeApiKey', () => revokeApiKey({ data: ID }), `/auth/api-keys/${ID}`],
    [
      'getPipelineJob',
      () => getPipelineJob({ data: ID }),
      `/pipeline/jobs/${ID}`,
    ],
    [
      'deletePipelineOutput',
      () => deletePipelineOutput({ data: { id: ID, deleteFile: true } }),
      `/pipeline/outputs/${ID}?delete_file=true`,
    ],
    [
      'getTemplateCredentialSource',
      () =>
        getTemplateCredentialSource({
          data: { id: ID, platform: 'BILIBILI' },
        }),
      `/credentials/templates/${ID}/source?platform=BILIBILI`,
    ],
    [
      'listLogFiles',
      () => listLogFiles({ data: { from: '2026-01-01', limit: 100 } }),
      '/logging/files?from=2026-01-01&limit=100',
    ],
  ])('%s builds the documented path', async (_name, invoke, expected) => {
    await expect(requestedPath(invoke)).resolves.toBe(expected);
  });

  it('lists log files without any filter', async () => {
    await expect(requestedPath(() => listLogFiles())).resolves.toBe(
      '/logging/files',
    );
  });

  it('lists pipeline presets without a query string when unfiltered', async () => {
    await expect(requestedPath(() => listPipelinePresets())).resolves.toBe(
      '/pipeline/presets',
    );
  });

  it('lists pipeline presets with the filters it was given', async () => {
    await expect(
      requestedPath(() =>
        listPipelinePresets({
          data: { search: 'remux', limit: 10, offset: 0 },
        }),
      ),
    ).resolves.toBe('/pipeline/presets?search=remux&limit=10&offset=0');
  });

  it('keeps a template lookup scoped when no platform is given', async () => {
    await expect(
      requestedPath(() => getTemplateCredentialSource({ data: { id: ID } })),
    ).resolves.toBe(`/credentials/templates/${ID}/source`);
  });
});

describe('server function identifier validation', () => {
  const unusable: Array<[string, string]> = [
    ['an empty identifier', ''],
    ['an over-long identifier', 'a'.repeat(513)],
  ];

  it.each(unusable)(
    'getStreamer rejects %s with no request',
    async (_l, id) => {
      await expectNoRequest(() => getStreamer({ data: id }));
    },
  );

  it.each(unusable)('getSession rejects %s with no request', async (_l, id) => {
    await expectNoRequest(() => getSession({ data: id }));
  });

  it.each(unusable)(
    'getPipelineJob rejects %s with no request',
    async (_l, id) => {
      await expectNoRequest(() => getPipelineJob({ data: id }));
    },
  );

  it('rejects a non-string identifier', async () => {
    await expectNoRequest(() => getStreamer({ data: 42 as unknown as string }));
  });

  it('rejects an empty identifier in either filter segment', async () => {
    await expectNoRequest(() =>
      deleteFilter({ data: { streamerId: '', filterId: 'f1' } }),
    );
    fetchBackendMock.mockClear();
    await expectNoRequest(() =>
      deleteFilter({ data: { streamerId: ID, filterId: '' } }),
    );
  });

  it('rejects a priority update with no priority', async () => {
    await expectNoRequest(() =>
      updateStreamerPriority({
        data: { id: ID, priority: undefined as never },
      }),
    );
  });

  it('rejects a payload that does not match the request schema', async () => {
    await expectNoRequest(() =>
      updateFilter({
        data: {
          streamerId: ID,
          filterId: 'f1',
          data: {
            filter_type: 'CATEGORY',
            config: { categories: 'not-a-list' },
          } as never,
        },
      }),
    );
  });
});

describe('server function identifier containment', () => {
  // An identifier that carries path or query syntax must stay inside its own
  // segment so it cannot redirect the request to a different endpoint.
  it.each([
    ['../admin', '/streamers/..%2Fadmin'],
    ['a/b', '/streamers/a%2Fb'],
    ['?limit=1', '/streamers/%3Flimit%3D1'],
    ['#frag', '/streamers/%23frag'],
    ['../../auth/api-keys', '/streamers/..%2F..%2Fauth%2Fapi-keys'],
  ])('getStreamer keeps %j in one segment', async (id, expected) => {
    await expect(requestedPath(() => getStreamer({ data: id }))).resolves.toBe(
      expected,
    );
  });

  it('keeps both filter segments contained', async () => {
    await expect(
      requestedPath(() =>
        deleteFilter({ data: { streamerId: '../x', filterId: 'a/b' } }),
      ),
    ).resolves.toBe('/streamers/..%2Fx/filters/a%2Fb');
  });
});

describe('server function request bodies', () => {
  // A validator that parses the payload must forward it unchanged: a schema
  // default reaching a partial update would overwrite a field the caller never
  // mentioned.
  it('forwards only the fields a streamer toggle changes', async () => {
    await expect(
      requestedBody(() =>
        updateStreamer({ data: { id: ID, data: { enabled: true } } }),
      ),
    ).resolves.toBe('{"enabled":true}');
  });

  it('forwards only the Douyin option a streamer override sets', async () => {
    await expect(
      requestedBody(() =>
        updateStreamer({
          data: {
            id: ID,
            data: {
              streamer_specific_config: { platform_extras: { ttwid: 'x' } },
            },
          },
        }),
      ),
    ).resolves.toBe(
      '{"streamer_specific_config":{"platform_extras":{"ttwid":"x"}}}',
    );
  });

  it('leaves settings out of a global config update that omits them', async () => {
    const body = await requestedBody(() =>
      updateGlobalConfig({ data: globalConfigWithoutToggles() }),
    );
    const sent = JSON.parse(body);
    expect(sent).not.toHaveProperty('auto_thumbnail');
    expect(sent).not.toHaveProperty('stream_proxy_allow_private_targets');
  });

  it('preserves an explicit streamer priority', async () => {
    await expect(
      requestedBody(() =>
        updateStreamer({ data: { id: ID, data: { priority: 'HIGH' } } }),
      ),
    ).resolves.toBe('{"priority":"HIGH"}');
  });

  // The channel editor submits `settings` as the object it built, which is what
  // the backend stores; the shared schema describes the serialized form.
  it('forwards channel settings given as an object', async () => {
    const settings = {
      webhook_url: 'https://example.test/hook',
      enabled: true,
    };
    await expect(
      requestedBody(() =>
        createChannel({
          data: {
            name: 'ops',
            channel_type: 'Discord',
            settings: settings as unknown as string,
          },
        }),
      ),
    ).resolves.toBe(
      JSON.stringify({ name: 'ops', channel_type: 'Discord', settings }),
    );
  });

  it('forwards channel settings given as a string', async () => {
    const settings = '{"a":1}';
    await expect(
      requestedBody(() =>
        createChannel({
          data: { name: 'ops', channel_type: 'Discord', settings },
        }),
      ),
    ).resolves.toBe(
      JSON.stringify({ name: 'ops', channel_type: 'Discord', settings }),
    );
  });

  it('adds the streamer id to a created filter', async () => {
    const body = {
      filter_type: 'CATEGORY',
      config: { categories: ['irl'] },
      streamer_id: ID,
    };
    await expect(
      requestedBody(() =>
        createFilter({
          data: {
            streamerId: ID,
            data: { filter_type: 'CATEGORY', config: { categories: ['irl'] } },
          },
        }),
      ),
    ).resolves.toBe(JSON.stringify(body));
  });

  it('forwards a pipeline preset definition unchanged', async () => {
    const preset = {
      name: 'remux only',
      dag: {
        name: 'remux only',
        steps: [
          {
            id: 'remux',
            step: { type: 'inline' as const, processor: 'remux', config: {} },
          },
        ],
      },
    };
    await expect(
      requestedBody(() => createPipelinePreset({ data: preset })),
    ).resolves.toBe(JSON.stringify(preset));
  });
});
