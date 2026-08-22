import { create } from 'zustand';
import { createJSONStorage, persist } from 'zustand/middleware';

import type { ImportedTheme } from '@/types/theme-customizer';

export type ThemeBase = 'preset' | 'imported';

export type ThemeSettingsState = {
  base: ThemeBase;
  preset: string;
  radius: string;
  overrides: Record<string, string>;
  importedTheme: ImportedTheme | null;

  setPreset: (preset: string) => void;
  setRadius: (radius: string) => void;
  setOverride: (cssVar: string, value: string) => void;
  clearOverride: (cssVar: string) => void;
  setImportedTheme: (theme: ImportedTheme | null) => void;
  setBase: (base: ThemeBase) => void;
  reset: () => void;
};

/** Persisted-fields subset of ThemeSettingsState (everything but actions). */
export type ThemeSettingsSnapshot = Pick<
  ThemeSettingsState,
  'base' | 'preset' | 'radius' | 'overrides' | 'importedTheme'
>;

/** Exported so ThemeSettingsSync can detect pristine settings (no user-theme
 *  <style> element needed — styles.css already carries these values, which
 *  theme-default-drift.test.ts enforces). */
export const DEFAULT_SETTINGS: ThemeSettingsSnapshot = {
  base: 'preset',
  preset: 'default',
  radius: '0.625rem',
  overrides: {},
  importedTheme: null,
};

/** localStorage key of the persisted store; ThemeSettingsSync listens for
 *  cross-tab `storage` events on it. */
export const STORAGE_KEY_SETTINGS = 'theme-settings';

function normalizeCssVar(cssVar: string): string {
  return cssVar.startsWith('--') ? cssVar.slice(2) : cssVar;
}

export const useThemeSettings = create(
  persist<ThemeSettingsState>(
    (set) => ({
      ...DEFAULT_SETTINGS,
      setPreset: (preset) => set({ preset, base: 'preset' }),
      setRadius: (radius) => set({ radius }),
      setOverride: (cssVar, value) => {
        const key = normalizeCssVar(cssVar);
        set((state) => {
          const next = { ...state.overrides };
          if (!value) {
            delete next[key];
          } else {
            next[key] = value;
          }
          return { overrides: next };
        });
      },
      clearOverride: (cssVar) => {
        const key = normalizeCssVar(cssVar);
        set((state) => {
          if (!(key in state.overrides)) return state;
          const next = { ...state.overrides };
          delete next[key];
          return { overrides: next };
        });
      },
      setImportedTheme: (theme) =>
        set((state) => ({
          importedTheme: theme,
          base: theme
            ? 'imported'
            : state.base === 'imported'
              ? 'preset'
              : state.base,
        })),
      setBase: (base) => set({ base }),
      reset: () => set(DEFAULT_SETTINGS),
    }),
    {
      name: STORAGE_KEY_SETTINGS,
      storage: createJSONStorage(() => localStorage),
      skipHydration: true,
    },
  ),
);
