import '@testing-library/jest-dom/vitest';

// Node 22+ exposes a built-in `localStorage` that shadows jsdom's Storage
// implementation.  When `--localstorage-file` is missing (always the case in
// vitest) the global is a plain object with **no** Storage methods.  Replace it
// with a spec-compliant in-memory implementation so component code that calls
// `localStorage.getItem` / `.setItem` / `.removeItem` works correctly.
{
  const store = new Map<string, string>();
  const storage: Storage = {
    get length() {
      return store.size;
    },
    clear() {
      store.clear();
    },
    getItem(key: string) {
      return store.get(key) ?? null;
    },
    key(index: number) {
      return [...store.keys()][index] ?? null;
    },
    removeItem(key: string) {
      store.delete(key);
    },
    setItem(key: string, value: string) {
      store.set(key, String(value));
    },
  };

  Object.defineProperty(globalThis, 'localStorage', {
    value: storage,
    writable: true,
    configurable: true,
  });
}

// jsdom does not implement window.matchMedia — provide a minimal mock. Files
// that opt into the node environment have no window to patch.
if (typeof window !== 'undefined') {
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: vi.fn().mockImplementation((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  });
}
