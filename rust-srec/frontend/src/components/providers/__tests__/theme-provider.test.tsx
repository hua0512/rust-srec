import { render, renderHook, act } from '@testing-library/react';
import { ThemeProvider, useTheme } from '../theme-provider';

// ThemeSettingsSync depends on a zustand store and is tested separately.
vi.mock('@/components/providers/theme-settings-sync', () => ({
  ThemeSettingsSync: () => null,
}));

/** Helper: mock matchMedia so that it reports a dark system preference. */
function mockSystemDark(dark: boolean) {
  const listeners: Array<(e: MediaQueryListEvent) => void> = [];
  const mql = {
    matches: dark,
    media: '(prefers-color-scheme: dark)',
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn((_: string, cb: (e: MediaQueryListEvent) => void) =>
      listeners.push(cb),
    ),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  };

  vi.mocked(window.matchMedia).mockReturnValue(
    mql as unknown as MediaQueryList,
  );

  /** Simulate a change event on the media query. */
  function fireChange(matches: boolean) {
    mql.matches = matches;
    for (const cb of listeners) {
      cb({ matches } as MediaQueryListEvent);
    }
  }

  return { mql, fireChange };
}

/** Wrapper that provides ThemeProvider for renderHook. */
function wrapper({ children }: { children: React.ReactNode }) {
  return <ThemeProvider>{children}</ThemeProvider>;
}

beforeEach(() => {
  // Reset DOM and storage between tests.
  document.documentElement.className = '';
  document.documentElement.style.colorScheme = '';
  localStorage.removeItem('theme');
  vi.restoreAllMocks();
  // Restore the default matchMedia mock from setup.ts (matches: false = light).
  vi.mocked(window.matchMedia).mockImplementation((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }));
});

describe('ThemeProvider', () => {
  it('reads stored theme from localStorage on first render (no FOUC)', () => {
    localStorage.setItem('theme', 'dark');

    render(
      <ThemeProvider>
        <span>child</span>
      </ThemeProvider>,
    );

    // The dark class must be present after the very first commit — no intermediate
    // "system" render that would cause a flash-of-unstyled-content.
    expect(document.documentElement).toHaveClass('dark');
    expect(document.documentElement).not.toHaveClass('light');
  });

  it('falls back to defaultTheme when localStorage is empty', () => {
    // Default is 'system'. With matchMedia returning matches: false (light),
    // the resolved class should be 'light'.
    render(
      <ThemeProvider>
        <span />
      </ThemeProvider>,
    );

    expect(document.documentElement).toHaveClass('light');
    expect(localStorage.getItem('theme')).toBeNull();
  });

  it('setMode("dark") applies .dark class and writes to localStorage', () => {
    const { result } = renderHook(() => useTheme(), { wrapper });

    act(() => result.current.setMode('dark'));

    expect(document.documentElement).toHaveClass('dark');
    expect(document.documentElement).not.toHaveClass('light');
    expect(localStorage.getItem('theme')).toBe('dark');
  });

  it('setMode("light") applies .light class and removes .dark', () => {
    localStorage.setItem('theme', 'dark');

    const { result } = renderHook(() => useTheme(), { wrapper });

    // Starts as dark (from localStorage).
    expect(document.documentElement).toHaveClass('dark');

    act(() => result.current.setMode('light'));

    expect(document.documentElement).toHaveClass('light');
    expect(document.documentElement).not.toHaveClass('dark');
    expect(localStorage.getItem('theme')).toBe('light');
  });

  it('setMode("system") writes to localStorage and resolves to dark from matchMedia', () => {
    localStorage.setItem('theme', 'light');
    mockSystemDark(true);

    const { result } = renderHook(() => useTheme(), { wrapper });

    // Starts as 'light' from localStorage.
    expect(document.documentElement).toHaveClass('light');

    act(() => result.current.setMode('system'));

    expect(localStorage.getItem('theme')).toBe('system');
    expect(document.documentElement).toHaveClass('dark');
  });

  it('setMode("system") resolves to light when system prefers light', () => {
    localStorage.setItem('theme', 'dark');
    mockSystemDark(false);

    const { result } = renderHook(() => useTheme(), { wrapper });

    // Starts as 'dark' from localStorage.
    expect(document.documentElement).toHaveClass('dark');

    act(() => result.current.setMode('system'));

    expect(localStorage.getItem('theme')).toBe('system');
    expect(document.documentElement).toHaveClass('light');
  });

  it('responds to system media query changes when mode is "system"', () => {
    const { fireChange } = mockSystemDark(false);

    const { result } = renderHook(() => useTheme(), { wrapper });

    // Ensure we're on system mode.
    act(() => result.current.setMode('system'));
    expect(document.documentElement).toHaveClass('light');

    // Simulate system switching to dark.
    act(() => fireChange(true));

    expect(document.documentElement).toHaveClass('dark');
    expect(document.documentElement).not.toHaveClass('light');
  });

  it('exposes resolvedMode alongside mode', () => {
    mockSystemDark(true);

    const { result } = renderHook(() => useTheme(), { wrapper });

    // Default mode is 'system', system is dark.
    expect(result.current.mode).toBe('system');
    expect(result.current.resolvedMode).toBe('dark');

    act(() => result.current.setMode('light'));

    expect(result.current.mode).toBe('light');
    expect(result.current.resolvedMode).toBe('light');
  });

  it('sets color-scheme CSS property on documentElement', () => {
    const { result } = renderHook(() => useTheme(), { wrapper });

    act(() => result.current.setMode('dark'));
    expect(document.documentElement.style.colorScheme).toBe('dark');

    act(() => result.current.setMode('light'));
    expect(document.documentElement.style.colorScheme).toBe('light');
  });

  it('falls back to the default mode when localStorage holds a garbage value', () => {
    localStorage.setItem('theme', 'purple');

    const { result } = renderHook(() => useTheme(), { wrapper });

    // Invalid stored value must never reach classList / color-scheme.
    expect(result.current.mode).toBe('system');
    expect(document.documentElement).toHaveClass('light');
    expect(document.documentElement).not.toHaveClass('purple');
  });

  it('ignores garbage cross-tab storage values', () => {
    localStorage.setItem('theme', 'dark');

    const { result } = renderHook(() => useTheme(), { wrapper });
    expect(result.current.mode).toBe('dark');

    act(() => {
      window.dispatchEvent(
        new StorageEvent('storage', { key: 'theme', newValue: 'purple' }),
      );
    });

    expect(result.current.mode).toBe('system');
    expect(document.documentElement).not.toHaveClass('purple');
  });

  it('hydrates from serverMode, then adopts localStorage and repairs the cookie', () => {
    // SSR divergence scenario: cookie says light, localStorage says dark.
    localStorage.setItem('theme', 'dark');

    const { result } = renderHook(() => useTheme(), {
      wrapper: ({ children }: { children: React.ReactNode }) => (
        <ThemeProvider serverMode="light">{children}</ThemeProvider>
      ),
    });

    // The mount reconcile effect adopts localStorage as the client authority
    // and rewrites the cookie so the next SSR pass agrees.
    expect(result.current.mode).toBe('dark');
    expect(document.documentElement).toHaveClass('dark');
    expect(document.cookie).toContain('theme=dark');
  });

  it('useTheme() returns default context value when used outside ThemeProvider', () => {
    // The context carries a default value, so consumers outside the provider
    // get inert defaults instead of a crash.
    const { result } = renderHook(() => useTheme());

    expect(result.current.mode).toBe('system');
    expect(result.current.resolvedMode).toBe('light');
    expect(result.current.setMode).toBeTypeOf('function');
  });
});
