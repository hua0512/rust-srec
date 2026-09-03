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
  beforeEach(() => {
    useAppSessionMock.mockResolvedValue({
      data: { token: { access_token: 'old-token' } },
      update: vi.fn(),
    });
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
});
