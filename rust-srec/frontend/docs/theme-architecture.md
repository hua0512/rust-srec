# Theme system — architecture review and improvement plan

Review date: 2026-07-13. **Status: implemented** (all phases; see §5 for the design each
change follows). Scope: everything under `rust-srec/frontend` that participates in
theme mode (light/dark/system) and theme customization (presets, imported themes, overrides,
radius), on both the web (SSR) and desktop (Tauri) builds.

---

## 1. Current architecture

### 1.1 Live modules

| Concern           | File                                                                                                                                  | Role                                                                                                                                                                                                                        |
| ----------------- | ------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Constants         | `src/lib/theme-config.ts`                                                                                                             | `MODES`, `DEFAULT_MODE`, storage/cookie keys                                                                                                                                                                                |
| Mode state        | `src/components/providers/theme-provider.tsx`                                                                                         | React context; reads `localStorage['theme']`; applies `.light`/`.dark` + `color-scheme` to `<html>` in a `useEffect`; OS `prefers-color-scheme` listener; cross-tab `storage` listener; persists to localStorage + cookie   |
| SSR mode          | `src/integrations/theme/theme-middleware.ts`                                                                                          | Request middleware: cookie → router context (`theme.mode`), refreshes cookie expiry                                                                                                                                         |
| Pre-paint script  | `src/lib/theme-script.ts`                                                                                                             | `themeScript()` is `.toString()`'d into an inline `<head>` script: applies mode class + restores cached CSS vars from `localStorage['theme-vars-cache']`                                                                    |
| Root document     | `src/routes/__root.tsx`                                                                                                               | Injects the blocking script via route `head()`; renders `<html class={serverMode}>` from cookie context; **desktop branch renders no `<head>`/`HeadContent` at all**                                                        |
| Token settings    | `src/store/theme-settings.ts`                                                                                                         | zustand + `persist` (`localStorage['theme-settings']`, `skipHydration: true`): `base`, `preset`, `radius`, `overrides`, `importedTheme`                                                                                     |
| Token application | `src/components/providers/theme-settings-sync.tsx`                                                                                    | After rehydration, resolves the full var map for the _resolved mode_ and writes it as ~40 **inline styles** on `documentElement`, diffing against the previous key set; writes the light+dark cache for the blocking script |
| Mode animation    | `src/hooks/use-circular-transition.ts`                                                                                                | View Transitions API circular reveal around the click point; `flushSync(() => setMode(...))` inside `startViewTransition`                                                                                                   |
| UI                | `src/components/sidebar/mode-toggle.tsx`, `src/routes/_authed/_dashboard/config/theme.lazy.tsx`                                       | Toggle button (site header + navbar); full customizer page                                                                                                                                                                  |
| Preset data       | `src/utils/shadcn-ui-theme-presets.ts` (12 presets), `src/config/theme-data.ts` (adapter), `src/config/theme-customizer-constants.ts` | Preset palettes and customizer metadata                                                                                                                                                                                     |

### 1.2 Data flow — web (SSR, Nitro server behind nginx)

```
request ──> themeMiddleware (cookie 'theme' → context.theme.mode)
        ──> SSR: <html class={cookie mode, 'system'→'light'}> + <link styles.css> + inline themeScript
browser ──> themeScript runs pre-paint:
              class/color-scheme from localStorage['theme']
              inline CSS vars from localStorage['theme-vars-cache'] (resolved mode only)
        ──> first paint (correct for returning users)
        ──> hydration: ThemeProvider state from localStorage (applyToDOM is a no-op)
        ──> ThemeSettingsSync: persist.rehydrate() → effect applies resolved vars inline + rewrites cache
```

### 1.3 Data flow — desktop (Tauri, hash-history SPA)

```
Tauri shows splash (desktop-loading.html, themed by prefers-color-scheme ONLY)
main window created hidden, loads index.desktop.html   ← no theme script, no critical CSS
main.desktop.tsx bootstrap():
    await locale resolution
    createRoot(rootEl).render(<RouterProvider/>)        ← async concurrent commit
    await notifyFrontendReady()                         ← emits 'rust-srec://frontend-ready'
src-tauri/src/lib.rs: on frontend-ready → main window .show(), splash .close()
...later: ThemeProvider useEffect adds .dark; ThemeSettingsSync effect applies vars
```

`window.show()` races React's commit, and the theme class/vars are applied in **passive
effects that run after paint** — so a dark-mode user gets at least one white frame, usually
more, on every launch.

---

## 2. Verdict

**Is this the best architecture?** The web mode layer is fundamentally sound — cookie-informed
SSR + blocking pre-paint script + class strategy is the same family as `next-themes` /
`remix-themes`, and the light+dark vars cache is a genuinely good idea. Three structural
weaknesses keep it from being "best":

1. **The desktop build has no pre-paint layer at all** (no blocking script, no critical CSS,
   window shown before first commit) — the only user-visible FOUC in the product, and it hits
   every dark-mode desktop launch.
2. **Tokens are applied as mode-resolved inline styles**, which forces JS to re-apply ~40 vars
   on every mode flip, permanently shadows the stylesheet defaults, couples the circular
   reveal to React's passive-effect flush timing, and requires the light+dark cache dance.
   Compiling tokens into one `<style>` element containing both `:root{}` and `.dark{}` blocks
   makes mode flips pure CSS and removes all of that.
3. **~4,000 lines of dead code** from a previous iteration (tweakcn sheet customizer) are
   still in the tree, including a duplicated hardcoded var list that has already drifted.

**Do we have FOUC ("TOUC") problems?**

| Surface                                        | Status                                                                                                                                                              |
| ---------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Web, returning user (any mode/preset)          | ✅ No flash — blocking script covers class + vars                                                                                                                   |
| Web, first visit, OS dark                      | ✅ No flash — script resolves `system` via `matchMedia` pre-paint                                                                                                   |
| Web, after a deploy that changed preset values | ⚠️ One paint with stale cached palette; values self-heal post-hydration, but properties the new settings no longer define stay orphaned for the whole session (F5b) |
| Desktop, dark mode user                        | ❌ **White flash on every launch** (no script + `frontend-ready` race + effects run post-paint)                                                                     |
| Desktop, splash → main handoff                 | ⚠️ Splash follows OS scheme, app follows saved mode — mismatch when they differ                                                                                     |

**TOCTOU-style races** (checked explicitly): the `frontend-ready` vs. first-commit race above
is the only harmful one. Benign ones worth knowing: cookie vs. localStorage divergence (server
renders `mode` from the cookie, client state comes from localStorage; they only diverge if the
user selectively clears one — React 19 recovers via hydration re-render), settings applied
post-rehydration (masked by the vars cache on web), and last-writer-wins on the vars cache
across tabs (both writers produce equivalent output). The `theme-settings` store has **no
cross-tab sync** though, while mode does — tabs can show different presets until reload.

---

## 3. Findings

Severity: 🔴 user-visible bug · 🟡 latent/robustness · ⚪ hygiene.

| #   | Sev | Finding                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| --- | --- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| F1  | 🔴  | **Desktop FOUC.** `index.desktop.html` has neither the theme script nor critical CSS; the desktop branch of `RootDocument` renders no `HeadContent`, so the route-level script never mounts. Theme class + vars land in passive effects after first paint.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| F2  | 🔴  | **`frontend-ready` emitted before first paint.** `main.desktop.tsx` emits right after `createRoot().render()` returns; `lib.rs` shows the window on that event. The window can be shown before React commits anything.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| F3  | 🔴  | **`ModeToggle` toggles from `mode`, not `resolvedMode`.** With `mode='system'` + OS dark, `mode === 'dark' ? 'light' : 'dark'` yields `'dark'` — the first click changes nothing visually (plus runs a 400 ms reveal to an identical frame). `src/components/sidebar/mode-toggle.tsx:27`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| F4  | 🟡  | **Circular reveal depends on `flushSync` flushing passive effects.** `applyToDOM` (class) and the settings-sync var swap both live in `useEffect`; the View Transition callback needs the DOM mutated before it returns. It works today, but it's an implicit contract with React's flush behavior. Applying the class synchronously in `setMode` (and making mode flips not require var swaps at all — F6) removes the coupling.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| F5  | 🟡  | **Vars cache has no version/identity, and boot-applied properties are never diffed away.** Two failure modes. (a) `theme-vars-cache` written by one build is replayed pre-paint by the next; after preset value changes users see one stale-palette paint. (b) Worse: the blocking script applies every cached property as inline styles, but `ThemeSettingsSync` starts its diff from an empty `prevKeysRef` (`theme-settings-sync.tsx:135`), so `syncCssVars` never removes a boot-applied property that is absent from the current resolved settings (a build dropped a key from a preset, cache and `theme-settings` written out of step, selective storage clearing). Such orphans persist as inline styles **for the entire session** — `writeVarsCache` rewrites the cache on the first sync run, so the _next_ load is clean, but a long-lived dashboard tab keeps e.g. a stale `--font-sans`/`--shadow-*` from an old imported theme until reload. |
| F6  | 🟡  | **Tokens are mode-resolved inline styles.** Every load (even pristine default settings) writes ~40 inline vars that permanently shadow `styles.css`; every mode flip re-applies the full set from JS; the blocking script only restores vars for the mode resolved _at that instant_ (an OS scheme flip before hydration shows wrong tokens).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| F7  | 🟡  | **Default palette has two sources of truth, and they have already drifted.** `styles.css` `:root`/`.dark` vs `shadcnThemePresets.default`: the preset defines `destructive-foreground` but `styles.css` doesn't, and `@theme inline` has no `--color-destructive-foreground` mapping — so the `text-destructive-foreground` utility used by `template-card.tsx:200`, `channel-card.tsx:230`, `pipeline-summary-card.tsx:195`, `workflow-card.tsx:199` is never generated by Tailwind v4. Those destructive buttons currently render with inherited text color.                                                                                                                                                                                                                                                                                                                                                                                              |
| F8  | 🟡  | **Unvalidated mode values.** `readStorage`, the blocking script, and the `storage` event handler accept any string; a corrupted `localStorage['theme']` (e.g. `"purple"`) becomes `classList.add('purple')` + `colorScheme='purple'` — neither `.light` nor `.dark` gets applied.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| F9  | 🟡  | **No cross-tab sync for `theme-settings`** (mode has it via the `storage` listener in `theme-provider.tsx`). Preset/override changes in one tab don't reach others.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| F15 | 🟡  | **Cookie and localStorage are competing mode authorities.** SSR renders `<html class>` and `theme.mode` context from the cookie (`theme-middleware.ts:13`); the blocking script and provider both read localStorage (`theme-provider.tsx:76`, `theme-script.ts:34`). If the cookie is cleared while localStorage remains, SSR renders `system` and the middleware keeps _refreshing that wrong cookie on every request_, so it never self-corrects. The blocking script repaints the correct class before first paint (no color FOUC, and `<html suppressHydrationWarning>` masks the class mismatch), but any control that reads `Route.useRouteContext().theme.mode` on the server vs `useTheme().mode` on the client hydrates from different values until a `setMode` rewrites the cookie.                                                                                                                                                               |
| F10 | ⚪  | **Splash page ignores the saved mode** — `desktop-loading.html` themes by `prefers-color-scheme` media query only.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| F11 | ⚪  | **Dead code, ~4,050 lines**: `src/utils/tweakcn-theme-presets.ts` (3,425 — zero imports), `src/hooks/use-theme-manager.ts` (188 — zero imports, contains the drifted hardcoded var list), `src/lib/disable-transitions.ts` (30), `src/components/layout/themes/theme-customizer.tsx` + its exclusive children `theme-tab.tsx` and `layout-tab.tsx` (sheet customizer superseded by `/config/theme` route), `src/store/theme-customizer.ts`. Also unused exports `toggleTheme`/`isTransitioning` in `use-circular-transition.ts`.                                                                                                                                                                                                                                                                                                                                                                                                                            |
| F12 | ⚪  | Selecting the already-active resolved appearance (e.g. `system` when OS matches the current explicit mode) still runs the 400 ms view transition to an identical frame.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| F13 | ⚪  | `useTheme`'s `throw` guard is unreachable (context has a default value) — documented by the test at `theme-provider.test.tsx:189`. Either drop the guard or default the context to `undefined`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| F14 | ⚪  | Root `pendingComponent` (`AppLoadingScreen`) hardcodes a dark slate design — light-mode users get a dark loading screen. Acceptable as branding; noted for completeness.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |

---

## 4. Target architecture

Keep the overall shape (context provider + cookie SSR + blocking script + zustand settings
store); rework the token layer and extend the pre-paint layer to desktop.

```
┌──────────────────────────────────────────────────────────────────────┐
│ Pre-paint (both platforms, ONE script source: lib/theme-script.ts)   │
│   web:     injected via __root head()                                │
│   desktop: injected into index.desktop.html + desktop-loading.html   │
│            by a Vite transformIndexHtml plugin                       │
│   does:    validated mode class + color-scheme;                      │
│            inject <style id="user-theme-vars"> from versioned cache  │
└──────────────────────────────────────────────────────────────────────┘
┌──────────────────────────────┐  ┌──────────────────────────────────┐
│ Mode layer (ThemeProvider)   │  │ Token layer (ThemeSettingsSync)  │
│  class + color-scheme only   │  │  settings → CSS text:            │
│  applied SYNCHRONOUSLY in    │  │    :root{light} .dark{dark}      │
│  setMode / listeners; effect │  │  one <style> element, kept last  │
│  remains as reconciliation   │  │  in <head>; versioned cache;     │
└──────────────────────────────┘  │  pristine settings → no element  │
                                  └──────────────────────────────────┘
```

Why the `<style>` element beats inline styles:

- **Mode flip = one class change.** The `.dark{}` block is already in the document, so the
  cascade swaps every token atomically with the class — no 40-property JS loop inside the
  view-transition window, no dependence on effect flush order (fixes F4/F6 structurally).
- **Pre-hydration OS scheme flips are covered** — both modes' vars are present from the
  blocking script (fixes the F6 edge).
- **The stylesheet defaults stay authoritative** for pristine settings; devtools shows real
  cascade instead of 40 inline props; "reset" = remove one element.
- **Cache is a single CSS string** with a build id — cheap to validate and apply, and the
  sync replaces the element's `textContent` wholesale, so there is no per-property
  bookkeeping through which boot-applied state can outlive the settings that produced it
  (fixes both halves of F5: stale values _and_ orphaned properties).

Cascade note: the user style element wins over `styles.css` by **selector specificity**, not
document order — `compileThemeCss` emits `:root:root:not(.dark){}` and `:root:root.dark{}`
(both 0-3-0), which outrank the stylesheet's `:root`/`.dark` (0-1-0) wherever the element
sits. Order-independence matters because the blocking script runs during parse: on desktop it
is injected at `head-prepend`, so Vite's stylesheet `<link>` does not exist in the DOM yet
when the element is appended. The two selectors are also **mutually exclusive**: imported
themes can define a var in one mode only (`import-modal.tsx` parses the `:root` and `.dark`
sections independently), and a light-only key must not match `html.dark` and override the
stylesheet's `.dark` fallback while dark mode is active.

**Alternatives considered and rejected**

- _Adopt `next-themes`_: our provider already mirrors its semantics; we'd still hand-roll the
  SSR cookie context, the vars cache, and the desktop injection. Not worth the dependency.
- _Cookie as the single mode store (drop localStorage)_: simplifies divergence away, but loses
  free cross-tab `storage` events and makes the blocking script parse `document.cookie`.
  Dual-write with validation is fine.
- _Generate `styles.css` `:root`/`.dark` from the default preset at build time_: fixes F7 more
  aggressively but adds a codegen step; a drift-guard test gives the same protection cheaper.

---

## 5. Implementation plan

Phases are independently shippable, ordered by user impact. Phase 0 is a prerequisite for the
"pristine settings → no style element" behavior in Phase 2.

### Phase 0 — reconcile default palette + drift guard (fixes F7)

1. `src/styles.css`: add to `:root`: `--destructive-foreground: oklch(0.985 0 0);` and to
   `.dark`: `--destructive-foreground: oklch(0.985 0 0);` (values from
   `shadcnThemePresets.default`). Add to `@theme inline`:
   `--color-destructive-foreground: var(--destructive-foreground);`.
2. Add `src/__tests__/theme-default-drift.test.ts`:

```ts
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { shadcnThemePresets } from '@/utils/shadcn-ui-theme-presets';

const css = readFileSync(join(__dirname, '../styles.css'), 'utf8');

function cssVars(block: string): Record<string, string> {
  const m = css.match(
    block === ':root'
      ? /^:root \{([\s\S]*?)\n\}/m
      : /^\.dark \{([\s\S]*?)\n\}/m,
  );
  const out: Record<string, string> = {};
  for (const [, k, v] of m![1].matchAll(/--([\w-]+):\s*([^;]+);/g))
    out[k] = v.trim();
  return out;
}

// Guards the "pristine settings need no injected vars" assumption in
// ThemeSettingsSync: every var the default preset would inject must already
// exist in styles.css with the same value.
describe('styles.css matches shadcnThemePresets.default', () => {
  for (const [mode, block] of [
    ['light', ':root'],
    ['dark', '.dark'],
  ] as const) {
    it(`${block} covers default preset ${mode} values`, () => {
      const fromCss = cssVars(block);
      const preset = shadcnThemePresets.default.styles[mode];
      for (const [key, value] of Object.entries(preset)) {
        if (key.startsWith('font-')) continue; // fonts are hardcoded on body in styles.css
        expect(fromCss[key], `--${key}`).toBe(value.replace(/\s+/g, ' '));
      }
    });
  }
});
```

_(If values legitimately need to differ someday, the preset is the one to change — it is what
users see in the picker as "Zinc".)_

### Phase 1 — mode layer correctness (fixes F3, F4-mode-half, F8, F12, F15-mitigation)

`src/lib/theme-config.ts` — add validation + shared system-theme read:

```ts
export function isMode(value: unknown): value is Mode {
  return MODES.includes(value as Mode);
}

export function getSystemTheme(): ResolvedMode {
  if (typeof window === 'undefined') return 'light';
  return window.matchMedia('(prefers-color-scheme: dark)').matches
    ? 'dark'
    : 'light';
}
```

`src/components/providers/theme-provider.tsx`:

```ts
const [mode, setModeState] = useState<Mode>(() => {
  const stored = readStorage(STORAGE_KEY_MODE, '');
  return isMode(stored) ? stored : (serverMode ?? DEFAULT_MODE);
});

const setMode = useCallback(
  (next: Mode) => {
    setModeState(next);
    // Mutate <html> synchronously so a document.startViewTransition callback
    // that wraps setMode captures the new theme without depending on when
    // React flushes the applyToDOM effect.
    applyToDOM(next === 'system' ? getSystemTheme() : next);
    writeStorage(STORAGE_KEY_MODE, next);
    writeCookie(COOKIE_KEY_MODE, next);
  },
  [applyToDOM],
);

// storage listener: validate before trusting another tab's value
const handler = (e: StorageEvent) => {
  if (e.key !== STORAGE_KEY_MODE) return;
  setModeState(isMode(e.newValue) ? e.newValue : DEFAULT_MODE);
};
```

The existing `useEffect(applyToDOM, [resolvedMode])` stays — it reconciles the
storage/media-query paths and is idempotent.

Also in `ThemeProvider`: re-align the cookie with localStorage once on mount, so a cleared or
lost cookie stops being refreshed with the wrong value by `themeMiddleware` on every request
(F15) and the _next_ SSR pass renders the same mode the client hydrated with:

```ts
// localStorage is the client authority; the cookie only exists so
// themeMiddleware can render the right <html class> on the next request.
// Rewrite it whenever they disagree (e.g. cookies cleared, storage kept).
useEffect(() => {
  if (serverMode !== undefined && serverMode !== mode) {
    writeCookie(COOKIE_KEY_MODE, mode);
  }
  // Runs once: `mode` changes only via setMode, which writes the cookie itself.
  // eslint-disable-next-line react-hooks/exhaustive-deps
}, []);
```

`src/hooks/use-circular-transition.ts` — expose one entry point that skips the animation when
the resolved appearance won't change, and use it from both call sites (`mode-toggle.tsx`,
`config/theme.lazy.tsx`); delete the unused `toggleTheme`/`isTransitioning`:

```ts
const setModeWithReveal = useCallback(
  (next: Mode, coords: { x: number; y: number }) => {
    const nextResolved = next === 'system' ? getSystemTheme() : next;
    if (nextResolved === resolvedMode) {
      setMode(next); // persist the choice; nothing visual to animate
      return;
    }
    startTransition(coords, () => setMode(next));
  },
  [resolvedMode, setMode, startTransition],
);
```

`src/components/sidebar/mode-toggle.tsx` — toggle from the _resolved_ mode:

```ts
const { resolvedMode } = useTheme();
const { setModeWithReveal } = useCircularTransition();
// onClick:
setModeWithReveal(resolvedMode === 'dark' ? 'light' : 'dark', {
  x: event.clientX,
  y: event.clientY,
});
```

### Phase 2 — token layer rework: style element + versioned cache (fixes F4, F5, F6)

`src/lib/theme-script.ts` — new cache shape `{ v: buildId, css: string }`, style-element
injection, validated mode. The function stays fully self-contained (it is `.toString()`'d):

```ts
import { DEFAULT_MODE, STORAGE_KEY_MODE } from './theme-config'; // relative: this module is
// also imported by vite.desktop.config.ts, where the '@/' alias does not resolve.

export const THEME_CSS_CACHE_KEY = 'theme-css-cache';
export const USER_THEME_STYLE_ID = 'user-theme-vars';
export const LEGACY_VARS_CACHE_KEY = 'theme-vars-cache';

/** Build id baked into the cache entry; a mismatch after an update makes the
 *  blocking script fall back to stylesheet defaults instead of replaying a
 *  palette compiled from old preset data. */
export function getThemeBuildId(): string {
  return import.meta.env?.VITE_UI_BUILD || import.meta.env?.MODE || 'dev';
}

function themeScript(
  storageKey: string,
  defaultMode: string,
  cacheKey: string,
  styleId: string,
  buildId: string,
) {
  var el = document.documentElement;
  var modes = ['light', 'dark', 'system'];

  function systemTheme() {
    try {
      return window.matchMedia('(prefers-color-scheme: dark)').matches
        ? 'dark'
        : 'light';
    } catch {
      return 'light';
    }
  }

  var mode = defaultMode;
  try {
    var stored = localStorage.getItem(storageKey);
    if (stored && modes.indexOf(stored) !== -1) mode = stored;
  } catch {
    // localStorage unavailable — keep defaultMode
  }

  var resolved = mode === 'system' ? systemTheme() : mode;
  el.classList.remove('light', 'dark');
  el.classList.add(resolved);
  el.style.colorScheme = resolved;

  try {
    var raw = localStorage.getItem(cacheKey);
    if (raw) {
      var entry = JSON.parse(raw);
      if (entry && entry.v === buildId && typeof entry.css === 'string') {
        var style = document.createElement('style');
        style.id = styleId;
        style.textContent = entry.css;
        // Appended at the current end of <head>: after the styles.css <link>,
        // so equal-specificity :root/.dark rules here win by document order.
        document.head.appendChild(style);
      }
    }
  } catch {
    // malformed cache — stylesheet defaults still apply
  }
}

export function buildThemeScriptHTML(
  buildId: string = getThemeBuildId(),
): string {
  const args = JSON.stringify([
    STORAGE_KEY_MODE,
    DEFAULT_MODE,
    THEME_CSS_CACHE_KEY,
    USER_THEME_STYLE_ID,
    buildId,
  ]);
  return `(${themeScript.toString()}).apply(null,${args})`;
}

/** Pre-stylesheet background so an unstyled frame is never white in dark mode.
 *  Values duplicate --background in styles.css :root/.dark (guarded by
 *  theme-default-drift.test.ts). */
export const CRITICAL_THEME_CSS =
  'html{background-color:oklch(1 0 0)}html.dark{background-color:oklch(0.141 0.005 285.823)}';
```

`src/components/providers/theme-settings-sync.tsx` — compile both modes into one element; no
more `useTheme()` dependency, per-var diffing, or resolved-mode branching:

```tsx
import * as React from 'react';
import { useShallow } from 'zustand/react/shallow';
import {
  getThemeBuildId,
  LEGACY_VARS_CACHE_KEY,
  THEME_CSS_CACHE_KEY,
  USER_THEME_STYLE_ID,
} from '@/lib/theme-script';
import { shadcnThemePresets } from '@/utils/shadcn-ui-theme-presets';
import {
  DEFAULT_SETTINGS,
  STORAGE_KEY_SETTINGS,
  useThemeSettings,
} from '@/store/theme-settings';

// resolveVars() and withSidebarVars() are unchanged from the current file.

function compileThemeCss(settings: Settings): string {
  const block = (vars: Record<string, string>) =>
    Object.entries(vars)
      .map(([key, value]) => `--${key}:${value};`)
      .join('');
  // .dark after :root — equal specificity, document order decides.
  return `:root{${block(resolveVars(settings, false))}}\n.dark{${block(resolveVars(settings, true))}}`;
}

function isPristine(s: Settings): boolean {
  return (
    s.base === DEFAULT_SETTINGS.base &&
    s.preset === DEFAULT_SETTINGS.preset &&
    s.radius === DEFAULT_SETTINGS.radius &&
    Object.keys(s.overrides).length === 0 &&
    s.importedTheme === null
  );
}

export function ThemeSettingsSync() {
  const settings = useThemeSettings(
    useShallow((state) => ({
      base: state.base,
      preset: state.preset,
      radius: state.radius,
      overrides: state.overrides,
      importedTheme: state.importedTheme,
    })),
  );
  const [hydrated, setHydrated] = React.useState(false);

  React.useEffect(() => {
    // One-time migration from the inline-style token system: drop the old
    // cache key and strip inline --* vars (they would otherwise permanently
    // shadow the user-theme <style> element). --x/--y belong to
    // use-circular-transition and must survive.
    try {
      localStorage.removeItem(LEGACY_VARS_CACHE_KEY);
    } catch {
      /* unavailable */
    }
    const root = document.documentElement;
    for (let i = root.style.length - 1; i >= 0; i--) {
      const prop = root.style[i];
      if (prop.startsWith('--') && prop !== '--x' && prop !== '--y') {
        root.style.removeProperty(prop);
      }
    }

    void useThemeSettings.persist.rehydrate();
    setHydrated(true);
  }, []);

  // Cross-tab: mirror what theme-provider does for the mode key.
  React.useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === STORAGE_KEY_SETTINGS)
        void useThemeSettings.persist.rehydrate();
    };
    window.addEventListener('storage', onStorage);
    return () => window.removeEventListener('storage', onStorage);
  }, []);

  React.useEffect(() => {
    if (!hydrated || typeof document === 'undefined') return;

    let el = document.getElementById(USER_THEME_STYLE_ID);

    if (isPristine(settings)) {
      el?.remove();
      try {
        localStorage.removeItem(THEME_CSS_CACHE_KEY);
      } catch {
        /* unavailable */
      }
      return;
    }

    const css = compileThemeCss(settings);
    if (!el) {
      el = document.createElement('style');
      el.id = USER_THEME_STYLE_ID;
    }
    if (el.textContent !== css) el.textContent = css;
    // Keep the element last in <head> so it stays after any stylesheet Vite
    // injects later (dev/HMR), winning equal-specificity cascade order.
    if (el !== document.head.lastElementChild) document.head.appendChild(el);

    try {
      localStorage.setItem(
        THEME_CSS_CACHE_KEY,
        JSON.stringify({ v: getThemeBuildId(), css }),
      );
    } catch {
      /* full or unavailable — blocking script falls back to defaults */
    }
  }, [hydrated, settings]);

  return null;
}
```

`src/store/theme-settings.ts` — export what the sync needs:

```ts
export const STORAGE_KEY_SETTINGS = 'theme-settings';
export const DEFAULT_SETTINGS = { ... };            // already exists; add `export`
// persist options: { name: STORAGE_KEY_SETTINGS, ... }
```

Semantics preserved from the current system: `overrides` and `radius` apply identically to
both modes (that is what `resolveVars` + the old cache already did).

### Phase 3 — desktop pre-paint + show-after-paint (fixes F1, F2, F10)

`vite.desktop.config.ts` — inject the same script + critical CSS into **both** HTML inputs
(`index.desktop.html` and `desktop-loading.html`):

```ts
import type { PluginOption } from 'vite';
import pkg from './package.json';
import {
  buildThemeScriptHTML,
  CRITICAL_THEME_CSS,
} from './src/lib/theme-script';

function themePrePaint(): PluginOption {
  // Must match getThemeBuildId() in the app bundle so cache entries written by
  // ThemeSettingsSync validate in the blocking script.
  const buildId = process.env.VITE_UI_BUILD || pkg.version;
  return {
    name: 'rust-srec:theme-pre-paint',
    transformIndexHtml: () => [
      { tag: 'style', children: CRITICAL_THEME_CSS, injectTo: 'head-prepend' },
      {
        tag: 'script',
        children: buildThemeScriptHTML(buildId),
        injectTo: 'head-prepend',
      },
    ],
  };
}
// plugins: [themePrePaint(), lingui(), ...]
```

> Cache-id note: the `__THEME_CACHE_ID__` define in both Vite configs is a **content hash**
> (`theme-cache-id.ts`) of `shadcn-ui-theme-presets.ts` + `theme-settings-sync.tsx` — the two
> modules the compiled css is a pure function of. The release id (`VITE_UI_BUILD`, set to
> `ref@sha` by release-rust-srec.yml) deliberately does NOT feed it: a release id would
> invalidate every user's cache on every release and cause a one-paint default-palette flash
> for customized users, while the content hash invalidates exactly when preset data or
> compile logic changes. The desktop config computes the hash once and feeds the same
> constant to both the define and the injected script, so the ids always match.

`desktop-loading.html` — the script now sets `.dark` from the saved mode (splash and main
window share the Tauri origin, hence the same `localStorage['theme']`), so switch the dark
palette from the media query to the class:

```css
/* was: @media (prefers-color-scheme: dark) { :root { ... } } */
html.dark {
  --bg-core: #09090b;
  --bg-surface: #18181b;
  --border-subtle: #27272a;
  --text-primary: #f4f4f5;
  --text-secondary: #a1a1aa;
  --accent-glow: rgba(59, 130, 246, 0.4);
  --logo-filter: invert(1) brightness(1.2);
}
```

(Users without a stored mode still get the right scheme — the script resolves `system` via
`matchMedia`.)

`src/main.desktop.tsx` — don't reveal the window until a frame has actually been presented:

```ts
createRoot(rootEl).render(
  <React.StrictMode>
    <RouterProvider router={router} />
  </React.StrictMode>,
);

// Two rAFs ≈ first frame presented. The inline script in index.desktop.html
// has already applied the theme class + critical background, so the frame the
// window is shown with (lib.rs shows it on 'rust-srec://frontend-ready') is
// correctly themed even if React has not finished committing.
await new Promise<void>((resolve) =>
  requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
);
await notifyFrontendReady();
```

Web keeps its existing injection path (`__root.tsx` `head()` → `buildThemeScriptHTML()`),
now sharing the new script implementation. Optionally add `CRITICAL_THEME_CSS` there too as a
`<style>` head entry — harmless belt-and-suspenders; the SSR HTML already carries the class.

### Phase 4 — dead code removal (fixes F11, F13)

Delete:

- `src/utils/tweakcn-theme-presets.ts`
- `src/hooks/use-theme-manager.ts`
- `src/lib/disable-transitions.ts`
- `src/components/layout/themes/theme-customizer.tsx`
- `src/components/layout/themes/theme-tab.tsx`
- `src/components/layout/themes/layout-tab.tsx`
- `src/store/theme-customizer.ts`

(All verified unimported: the sheet customizer trio is only reachable from
`theme-customizer.tsx`, which nothing imports; `/config/theme` route + `ImportModal` +
`ThemePresetPicker` are the live customizer UI.)

Also: remove the unreachable `throw` in `useTheme` (context has a default value; the test at
`theme-provider.test.tsx:189` already documents this), and drop the corresponding test.

### Phase 5 — tests

1. **Keep** all existing `theme-provider.test.tsx` behavior tests (they pass unchanged apart
   from the removed-guard test); add: invalid `localStorage['theme']` falls back to
   `DEFAULT_MODE`; invalid cross-tab `storage` value falls back.
2. **New `theme-script.test.ts`** — `eval(buildThemeScriptHTML('test-build'))` in jsdom:
   - applies `.dark` for stored `dark`; resolves `system` via mocked `matchMedia`;
   - ignores garbage stored mode;
   - injects `#user-theme-vars` when cache `{v:'test-build', css}` matches;
   - skips injection on version mismatch or malformed JSON without throwing.
3. **New `theme-settings-sync.test.tsx`** — with a seeded store: creates/updates the style
   element (`.dark` block after `:root`), removes it + cache when settings return to
   `DEFAULT_SETTINGS`, strips legacy inline `--*` vars but preserves `--x`/`--y`, rehydrates
   on a `theme-settings` storage event.
4. **Phase 0 drift-guard test** (above).

### Phase 6 — manual verification checklist

- Web SSR: hard-reload with `dark` + non-default preset → no flash (throttle CPU in devtools);
  first visit with OS dark → dark first paint; cookie deleted but localStorage kept → correct
  paint, no hydration errors in console.
- Desktop: launch with `dark` saved → splash dark, main window appears dark with no white
  frame (test with `RUST_SREC_DESKTOP_DEVTOOLS` build, and on a cold start); saved mode
  `light` + OS dark → splash and app both light.
- Mode toggle: `system` + OS dark → first click switches to light (F3); circular reveal
  animates in both directions; `prefers-reduced-motion` skips the animation; selecting
  `system` when it resolves to the current appearance does not animate (F12).
- Customizer: preset change updates instantly and survives reload with no flash; import →
  clear → reset returns to stylesheet defaults with **no** `#user-theme-vars` element and no
  inline `--*` styles on `<html>`; second tab picks up preset changes.
- After a simulated deploy (bump build id): reload with custom preset → no stale-palette
  paint from the old cache (falls back to defaults for one paint only if the preset itself
  changed, then corrects).

---

## 6. Rollout notes

- Ship Phase 0 + 1 together (pure fixes, no storage changes). Phase 2 changes the cache key
  (`theme-vars-cache` → `theme-css-cache`); the one-time cleanup in `ThemeSettingsSync`
  removes the old key and legacy inline vars, so mixed-version clients degrade to "no
  pre-paint vars for one load", never to wrong colors.
- Phase 3 touches the desktop show-window handshake; verify on Windows (primary target) that
  the double-rAF does not measurably delay `window.show()` — it should be ≤ 2 frames after
  the current timing, and the splash covers the gap regardless.
- No backend or protocol changes anywhere in the plan.

---

## 7. Cross-team review reconciliation (2026-07-13)

A second team reviewed the same code independently. Verification outcome per finding, and
what changed in this plan as a result:

| Their finding                                                                                      | Verified?                     | Disposition                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| -------------------------------------------------------------------------------------------------- | ----------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Desktop has no guaranteed flash-free initialization                                                | ✅ Correct                    | Already F1/F2; fixed by Phase 3                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| Vars cache can leave stale variables applied; runtime differ starts from an empty previous-key set | ✅ Correct, one overstatement | **New substance — F5 strengthened.** The orphan mechanism (`prevKeysRef` starts empty at `theme-settings-sync.tsx:135`, so boot-applied properties outside the current resolved set are never removed) was missing from the first draft. Correction: orphans last the _session_, not "permanently" — `writeVarsCache` rewrites the cache on the first sync run, so the next load boots clean. Phase 2's atomic style element removes the mechanism entirely; no plan change needed beyond the doc |
| Cookie and localStorage are competing authorities                                                  | ✅ Correct mechanism          | Now F15. Their sharpest point: the middleware _refreshes the wrong cookie forever_ after divergence. Mitigated by the one-time cookie re-align in Phase 1 rather than by changing authority (see below)                                                                                                                                                                                                                                                                                           |
| `ModeToggle` compares `mode` instead of `resolvedMode`                                             | ✅ Correct                    | Already F3; fixed in Phase 1                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| Persisted modes cast, not validated                                                                | ✅ Correct                    | Already F8; fixed in Phase 1                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| Two customizer architectures, one dead                                                             | ✅ Correct but incomplete     | Already F11 — their list misses the largest dead file (`tweakcn-theme-presets.ts`, 3,425 lines) and `disable-transitions.ts`; Phase 4 covers both lists                                                                                                                                                                                                                                                                                                                                           |

Findings in this document the review did not surface: F7 (`text-destructive-foreground`
utility never generated — user-visible today), F9 (no cross-tab settings sync — the
multi-tab variant of their stale-vars concern), F10 (splash ignores saved mode), F12 (no-op
reveal animations), F13/F14 (minor).

**Their proposed consolidation — adopt/reject:**

1. _"One validated, versioned persisted theme snapshot."_ **Partially adopted.** The derived
   cache becomes one versioned snapshot (`theme-css-cache`, Phase 2), and mode values get
   validated (Phase 1). Merging _mode_ into the zustand settings snapshot is rejected: mode
   and settings have different lifecycles (mode flips often, needs synchronous DOM
   application inside `startViewTransition`, cross-tab propagation, and a cookie mirror for
   SSR; settings change rarely and are zustand-persisted). Two localStorage reads in the
   blocking script cost nothing; coupling them would.
2. _"Cookie as the web mode authority."_ **Rejected.** It fixes a rare, self-healing,
   cosmetic divergence at the cost of: losing free cross-tab sync (`storage` events have no
   portable cookie equivalent — `cookieStore` change events are Chromium-only), forking mode
   persistence per platform (desktop has no cookie-reading server; it would stay on
   localStorage), and a clunkier blocking script (`document.cookie` parsing). Instead:
   localStorage stays the client authority, the cookie stays an SSR hint, and Phase 1 adds
   validation plus a one-time mount-time cookie re-align so divergence heals on the next
   request instead of being refreshed forever (F15).
3. _"Shared synchronous bootstrap/apply for web and desktop."_ **Already in plan** — Phase 3
   injects the same `buildThemeScriptHTML()` on both platforms.
4. _"Atomic or identity-checked variable cache recording applied property names."_
   **Superseded.** The Phase 2 style element is replaced atomically, which makes recording
   property names unnecessary; identity is the build id.
5. _"Desktop window readiness only after synchronous theme bootstrap."_ **Already in plan** —
   Phase 3 (inline script runs pre-paint; `frontend-ready` deferred past the first presented
   frame).
6. _"Remove the unused customizer implementation."_ **Already in plan** — Phase 4, with the
   two additional dead files noted above.

Severity calibration: their P2 for the cookie/localStorage split reads high — after Phase 1
lands (validation + re-align), what remains is a hydration re-render of a few mode-dependent
controls in a rare storage-clearing scenario; we track it as 🟡 with a cheap mitigation
rather than an architecture change.

### 7.1 Second review round (post-implementation) — all five findings fixed

| Their finding                                                                                    | Verified?                                                                                                                                                                                           | Fix applied                                                                                                                                                                                                                                                                                                                                                            |
| ------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [P1] Unhandled view-transition rejection reaches `renderFatal` on desktop                        | ✅ Correct — `ready.then(...)` had no rejection handler and `main.desktop.tsx` escalates every unhandled rejection to the fatal boot screen                                                         | `use-circular-transition.ts` now `.catch`es `ready` (skipped reveals: the mode change already committed, nothing to recover) and releases the lock via `finished.catch(() => {}).finally(...)`, which settles on every path                                                                                                                                            |
| [P2] Desktop cached css inserted before the stylesheet link, losing equal-specificity ties       | ✅ Correct — verified in the built `dist-desktop/index.desktop.html`: the pre-paint script precedes the css `<link>`, so the parse-time `appendChild` landed the user style first in document order | Order-independence via specificity: `compileThemeCss` emits `:root:root{}` / `:root:root.dark{}` (see cascade note in §4); the sync's re-append-to-end dance was removed as obsolete                                                                                                                                                                                   |
| [P2] Every release invalidates the theme cache (release workflow sets `VITE_UI_BUILD=ref@sha`)   | ✅ Correct — the timestamp/release-id derivation guaranteed a one-paint default flash per release for customized users                                                                              | `__THEME_CACHE_ID__` is now a content hash of the theming sources (`theme-cache-id.ts`), identical across web/desktop builds (verified: same hash in the SSR bundle and the desktop injected script) and stable across non-theming releases                                                                                                                            |
| [P2] Cookie re-align does not prevent the current request's hydration mismatch                   | ✅ Correct — the mount effect only healed the _next_ request                                                                                                                                        | On the SSR path the provider now initializes state from `serverMode` (hydration matches server markup by construction), then a mount effect adopts localStorage as the client authority and repairs the cookie. The desktop branch of `__root.tsx` no longer passes `serverMode`, so the SPA keeps reading localStorage directly and never fights the pre-paint script |
| [P2] Double-rAF from a hidden Tauri window may never fire → 6s `lib.rs` fallback on every launch | ✅ Correct risk — hidden webviews can suspend rAF (WKWebView pauses it), and `lib.rs:573` has the 6s show-anyway timer                                                                              | `main.desktop.tsx` now awaits a first-commit signal from a `useEffect` (fires regardless of visibility), then races the double-rAF against a 120 ms timeout — bounded worst case, and the pre-paint script guarantees the revealed frame is themed either way                                                                                                          |

### 7.2 Third review round — both findings fixed

| Their finding                                                                                                                                                                                                                                                 | Verified?                                                                                                                                | Fix applied                                                                                                                                                                                                                                                                                                                                   |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [P2] Asymmetric imported themes leak light values into dark mode: `:root:root` still matches `html.dark`, so a light-only key (import-modal enforces no key symmetry) outranks the stylesheet's `.dark` fallback                                              | ✅ Correct — `import-modal.tsx` parses the `:root`/`.dark` sections into independent maps, and 0-2-0 beats 0-1-0 regardless of the class | `compileThemeCss` now emits **mutually exclusive** selectors, `:root:root:not(.dark)` and `:root:root.dark` (both 0-3-0): a light-only key simply does not apply in dark mode, falling back through the stylesheet cascade exactly like the original per-mode inline-style system did. Asymmetric-import test added                           |
| [P2] SSR reconciliation can transiently reapply the stale cookie theme: the `applyToDOM` passive effect fires with the cookie-derived mode post-paint, before the (also passive) reconcile effect adopts localStorage → dark→light→dark flicker on divergence | ✅ Correct — both effects were passive, leaving a paintable window between them                                                          | Both the reconcile effect and the `applyToDOM` effect now run as layout effects (`useIsomorphicLayoutEffect` — server-safe), with reconcile declared first: `setModeState` commits and every `<html>` write completes before the browser paints, so the class set by the pre-paint script is never visibly replaced by the stale cookie value |

### 7.3 Fourth review round — cache/DOM write path consolidated

Two findings about the derived-cache write path:

**Claim A — one-paint default flash after a cache-id change (customized users).** True but working
as designed, and _not_ fixable the way suggested. The content hash changes precisely because the
preset data or compile logic changed, so the persisted css may be in the old format — replaying it
to avoid the flash is how wrong themes would ship after a compiler change. Putting a full compiler
in the blocking `<head>` script trades a rare one-paint flash for permanent weight in the most
performance-critical script. The content hash already minimizes the blast radius (invalidates only
when theming actually changes, never on unrelated releases). No change; accepted tradeoff.

**Claim B — stale css replayed if settings persist just before the cache effect runs.** Real, narrow
race: zustand `persist` writes `theme-settings` synchronously inside each setter, but the css cache
was written in a passive effect, so a reload in that sub-frame window paired new settings with old
css for one paint. Fixed by writing the cache from a module-level `useThemeSettings.subscribe`
(fires within the same `setState` as the persist write), guarded by `typeof window` for SSR.

While implementing Claim B, a deep review of the whole engine flagged the interactive hot path
(color-picker drag → `setOverride` fires continuously): the split between a subscription (cache) and
a `useShallow` render effect (`<style>`) meant every change ran `compileThemeCss` **twice**, wrote
localStorage twice, and re-rendered `ThemeSettingsSync`. Consolidated: a single
`applyThemeSideEffects(settings)` compiles once and drives both the `<style>` element and the cache,
driven entirely by the store subscription. `ThemeSettingsSync` no longer subscribes to settings, so
a drag updates the DOM synchronously with no React render and no double compile — and the cache css
is byte-identical to the live `<style>` (single source), making DOM/cache divergence impossible.

Deep-review items deliberately left for separate work (documented, not fixed here):

- **Overrides apply to both light and dark modes** (`resolveVars` spreads `overrides` into both
  blocks) and the native `<input type=color>` only emits hex while presets are oklch. Pre-existing
  behavior; a per-mode override model + oklch-aware picker is a feature change, not a bug fix.
- **Build-time static preset CSS + `data-theme` switching** would remove runtime preset compilation
  entirely (compile only imported themes/overrides at runtime). The single biggest simplification
  available, but a real rewrite — tracked as a future structural option, not part of this work.
