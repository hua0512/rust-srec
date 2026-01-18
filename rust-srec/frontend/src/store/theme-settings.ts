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

const DEFAULT_SETTINGS: Pick<
  ThemeSettingsState,
  'base' | 'preset' | 'radius' | 'overrides' | 'importedTheme'
> = {
  base: 'preset',
  preset: 'default',
  radius: '0.625rem',
  overrides: {},
  importedTheme: null,
};

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
      name: 'theme-settings',
      storage: createJSONStorage(() => localStorage),
      skipHydration: true,
    },
  ),
);
