import { SIDEBAR_COOKIE_KEY } from '@/lib/sidebar-cookie';
import { sidebarMiddleware } from './sidebar-middleware';

type SidebarContext = { sidebar: { open: boolean } };

async function runWith(cookie?: string) {
  const next = vi.fn(async (opts: { context: SidebarContext }) => opts);
  const server = sidebarMiddleware.options.server as unknown as (args: {
    request: Request;
    next: typeof next;
  }) => Promise<unknown>;

  await server({
    request: new Request('https://srec.test/dashboard', {
      headers: cookie ? { cookie } : undefined,
    }),
    next,
  });

  return next.mock.calls[0][0].context.sidebar.open;
}

describe('sidebarMiddleware', () => {
  it('hands the stored state to the router so the server renders it', async () => {
    await expect(runWith(`${SIDEBAR_COOKIE_KEY}=false`)).resolves.toBe(false);
    await expect(
      runWith(`theme_mode=dark; ${SIDEBAR_COOKIE_KEY}=true`),
    ).resolves.toBe(true);
  });

  it('falls back to expanded for a visitor with no stored state', async () => {
    await expect(runWith()).resolves.toBe(true);
    await expect(runWith('theme_mode=dark')).resolves.toBe(true);
  });
});
