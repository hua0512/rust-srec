import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

import { shadcnThemePresets } from '@/utils/shadcn-ui-theme-presets';

// vitest's root is the frontend package dir (where vitest.config.ts lives).
const css = readFileSync(resolve(process.cwd(), 'src/styles.css'), 'utf8');

function parseBlock(selector: ':root' | '.dark'): Record<string, string> {
  const re =
    selector === ':root'
      ? /^:root \{([\s\S]*?)\n\}/m
      : /^\.dark \{([\s\S]*?)\n\}/m;
  const match = css.match(re);
  if (!match) throw new Error(`${selector} block not found in styles.css`);

  const vars: Record<string, string> = {};
  for (const [, key, value] of match[1].matchAll(/--([\w-]+):\s*([^;]+);/g)) {
    vars[key] = value.trim().replace(/\s+/g, ' ');
  }
  return vars;
}

/** Resolve one level of `var(--x)` indirection (e.g. `--sidebar: var(--background)`). */
function resolveVar(value: string, scope: Record<string, string>): string {
  const m = value.match(/^var\(--([\w-]+)\)$/);
  return m ? (scope[m[1]] ?? value) : value;
}

const rootVars = parseBlock(':root');
// With the dark class active the effective value is the .dark override when
// present, else the :root value — mirror that cascade here.
const darkVars = { ...rootVars, ...parseBlock('.dark') };

// ThemeSettingsSync removes the user-theme <style> element entirely when
// settings equal DEFAULT_SETTINGS, on the assumption that the stylesheet
// already provides every value the default preset would inject. This test is
// what makes that assumption safe: any key/value drift between
// shadcnThemePresets.default and styles.css fails here.
describe('styles.css stays in sync with shadcnThemePresets.default', () => {
  const cases = [
    ['light', rootVars],
    ['dark', darkVars],
  ] as const;

  for (const [mode, cssVars] of cases) {
    it(`default preset ${mode} values match the stylesheet`, () => {
      const preset = shadcnThemePresets.default.styles[mode];
      for (const [key, value] of Object.entries(preset)) {
        const actual = resolveVar(cssVars[key] ?? '<missing>', cssVars);
        expect(actual, `--${key} (${mode})`).toBe(
          String(value).replace(/\s+/g, ' '),
        );
      }
    });
  }

  it('maps --color-destructive-foreground in @theme inline for the text-destructive-foreground utility', () => {
    expect(css).toMatch(
      /--color-destructive-foreground:\s*var\(--destructive-foreground\)/,
    );
  });
});
