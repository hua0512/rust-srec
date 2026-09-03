import { BackendApiError } from '@/lib/api-error';
import { fetchBackend } from '../api';

const refreshAuthTokenGlobalMock = vi.hoisted(() => vi.fn());
const useAppSessionMock = vi.hoisted(() => vi.fn());

vi.mock('../tokenRefresh', () => ({
  refreshAuthTokenGlobal: refreshAuthTokenGlobalMock,
}));

vi.mock('@/utils/session.server', () => ({
  useAppSession: useAppSessionMock,
}));

vi.mock('@/utils/env', () => ({
  BASE_URL: 'http://backend.test',
}));

describe('fetchBackend', () => {
  let session: {
    data: unknown;
    update: ReturnType<typeof vi.fn>;
    clear: ReturnType<typeof vi.fn>;
  };

  beforeEach(() => {
    session = {
      data: { token: { access_token: 'old-token' } },
      update: vi.fn(),
      clear: vi.fn(),
    };
    useAppSessionMock.mockResolvedValue(session);
    refreshAuthTokenGlobalMock.mockResolvedValue({
      status: 'refreshed',
      accessToken: 'new-token',
    });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it('preserves the retried response error after a successful refresh', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(new Response(null, { status: 401 }))
      .mockResolvedValueOnce(
        Response.json(
          { detail: 'invalid request' },
          { status: 422, statusText: 'Unprocessable Entity' },
        ),
      );
    vi.stubGlobal('fetch', fetchMock);

    const request = fetchBackend('/resource');

    await expect(request).rejects.toMatchObject({
      status: 422,
      body: { detail: 'invalid request' },
    } satisfies Partial<BackendApiError>);
    const retryHeaders = fetchMock.mock.calls[1][1]?.headers as Headers;
    expect(retryHeaders.get('Authorization')).toBe('Bearer new-token');
  });

  it.each(['transient', 'rejected'])(
    'reports the original 401 without retrying when the refresh is %s',
    async (status) => {
      refreshAuthTokenGlobalMock.mockResolvedValue({ status });
      const fetchMock = vi
        .fn()
        .mockResolvedValue(
          Response.json({ detail: 'unauthorized' }, { status: 401 }),
        );
      vi.stubGlobal('fetch', fetchMock);

      await expect(fetchBackend('/resource')).rejects.toMatchObject({
        status: 401,
      } satisfies Partial<BackendApiError>);
      expect(fetchMock).toHaveBeenCalledTimes(1);
      // Clearing the session is refreshAuthTokenGlobal's decision alone.
      expect(session.clear).not.toHaveBeenCalled();
    },
  );
});
