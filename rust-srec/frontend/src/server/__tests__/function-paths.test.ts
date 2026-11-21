import { updateGlobalConfig, updatePlatformConfig } from '../functions/config';
import { createChannel } from '../functions/notifications';
import { testProxy, updateProxy } from '../functions/proxies';
import { updateStreamer } from '../functions/streamers';

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
  // that parse fail and swallow it in the request helpers.
  fetchBackendMock.mockResolvedValue(undefined);
});

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
    output_retention_days: 0,
    output_retention_delete_files: false,
    log_filter_directive: 'info',
    pipeline_cpu_job_timeout_secs: 1,
    pipeline_io_job_timeout_secs: 1,
    pipeline_execute_timeout_secs: 1,
    queue_freshness_threshold_ms: 0,
    gpu_health_probe_interval_secs: 1,
  };
}

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

  it('leaves settings out of a global config update that omits them', async () => {
    const body = await requestedBody(() =>
      updateGlobalConfig({ data: globalConfigWithoutToggles() }),
    );
    const sent = JSON.parse(body);
    expect(sent).not.toHaveProperty('auto_thumbnail');
    expect(sent).not.toHaveProperty('stream_proxy_allow_private_targets');
  });

  it('sends routes as objects, never the replaced proxy_config', async () => {
    const global = JSON.parse(
      await requestedBody(() =>
        updateGlobalConfig({
          data: {
            ...globalConfigWithoutToggles(),
            proxy_route: { kind: 'proxy', id: ID },
          },
        }),
      ),
    );
    expect(global.proxy_route).toEqual({ kind: 'proxy', id: ID });
    expect(global).not.toHaveProperty('proxy_config');
    fetchBackendMock.mockClear();
    const platform = JSON.parse(
      await requestedBody(() =>
        updatePlatformConfig({
          data: {
            id: ID,
            data: {
              output_folder: null,
              output_filename_template: null,
              output_file_format: null,
              download_engine: null,
              proxy_route: { kind: 'system' },
            },
          },
        }),
      ),
    );
    expect(platform.proxy_route).toEqual({ kind: 'system' });
    expect(platform).not.toHaveProperty('proxy_config');
  });

  it('keeps a saved proxy password unless one is given', async () => {
    await expect(
      requestedBody(() =>
        updateProxy({
          data: { id: ID, expected_version: 2, username: null },
        }),
      ),
    ).resolves.toBe('{"expected_version":2,"username":null}');
  });

  it('refuses a proxy check without exactly one target', async () => {
    await expectNoRequest(() => testProxy({ data: { proxy_id: ID } }));
    await expectNoRequest(() =>
      testProxy({
        data: {
          proxy_id: ID,
          platform: 'bilibili',
          target_url: 'https://example.com',
        },
      }),
    );
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
});
