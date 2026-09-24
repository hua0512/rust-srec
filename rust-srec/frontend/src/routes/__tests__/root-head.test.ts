import { buildThemeScriptHTML } from '@/lib/theme-script';
import { Route } from '../__root';

describe('root route head()', () => {
  it('opts the pre-paint theme script out of Cloudflare Rocket Loader', async () => {
    const head = Route.options.head as () => Promise<{
      scripts?: Array<Record<string, unknown> | undefined>;
    }>;
    const { scripts = [] } = await head();
    const themeScript = scripts.find(
      (script) => script?.children === buildThemeScriptHTML(),
    );

    expect(themeScript).toBeDefined();
    // Without it, Rocket Loader defers the script past first paint and the
    // page shows the wrong mode and palette until it runs.
    expect(themeScript?.['data-cfasync']).toBe('false');
  });
});
