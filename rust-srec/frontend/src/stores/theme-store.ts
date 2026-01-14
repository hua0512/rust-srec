import { create } from 'zustand';
import { persist } from 'zustand/middleware';

export type ThemeColor =
  | 'zinc'
  | 'slate'
  | 'stone'
  | 'gray'
  | 'neutral'
  | 'red'
  | 'rose'
  | 'orange'
  | 'green'
  | 'blue'
  | 'yellow'
  | 'violet'
  | 'teal'
  | 'cyan'
  | 'indigo'
  | 'pink'
  | 'purple'
  | 'fuchsia'
  | 'emerald'
  | 'sky'
  | 'lime'
  | 'amber';
export type ThemeRadius = 0 | 0.3 | 0.5 | 0.625 | 0.75 | 1.0;

interface ThemeState {
  themeColor: ThemeColor;
  radius: ThemeRadius;
  customCss: string;
  isCustomCssEnabled: boolean;
  isGlassEnabled: boolean;

  setThemeColor: (color: ThemeColor) => void;
  setRadius: (radius: ThemeRadius) => void;
  setCustomCss: (css: string) => void;
  setIsCustomCssEnabled: (enabled: boolean) => void;
  setIsGlassEnabled: (enabled: boolean) => void;
}

export const useThemeStore = create<ThemeState>()(
  persist(
    (set, get) => ({
      themeColor: 'zinc',
      radius: 0.625,
      customCss: '',
      isCustomCssEnabled: false,
      isGlassEnabled: false,

      setThemeColor: (themeColor) => {
        set({ themeColor });
        updateThemeColor(themeColor);
      },
      setRadius: (radius) => {
        set({ radius });
        updateRadius(radius);
      },
      setCustomCss: (customCss) => {
        set({ customCss });
        if (get().isCustomCssEnabled) {
          updateCustomCss(customCss, true);
        }
      },
      setIsCustomCssEnabled: (isCustomCssEnabled) => {
        set({ isCustomCssEnabled });
        updateCustomCss(get().customCss, isCustomCssEnabled);
      },
      setIsGlassEnabled: (isGlassEnabled) => {
        set({ isGlassEnabled });
        updateGlassMode(isGlassEnabled);
      },
    }),
    {
      name: 'app-theme-storage',
      skipHydration: true,
      onRehydrateStorage: () => () => {
        // We no longer apply themes immediately during rehydration
        // to avoid hydration mismatches. The ThemeProvider will handle it.
        console.log('[ThemeStore] Rehydrated theme state');
      },
    },
  ),
);

// Side Effects Helpers
export function applyTheme(state: {
  themeColor: ThemeColor;
  radius: number;
  customCss: string;
  isCustomCssEnabled: boolean;
  isGlassEnabled: boolean;
}) {
  updateThemeColor(state.themeColor);
  updateRadius(state.radius);
  updateCustomCss(state.customCss, state.isCustomCssEnabled);
  updateGlassMode(state.isGlassEnabled);
}

export function updateThemeColor(color: ThemeColor) {
  if (typeof document !== 'undefined') {
    document.body.setAttribute('data-theme-color', color);
  }
}

export function updateRadius(radius: number) {
  if (typeof document !== 'undefined') {
    document.body.style.setProperty('--radius', `${radius}rem`);
  }
}

export function updateCustomCss(css: string, enabled: boolean) {
  if (typeof document !== 'undefined') {
    const styleId = 'custom-theme-style';
    const existingStyle = document.getElementById(styleId);

    if (!enabled) {
      existingStyle?.remove();
      return;
    }

    if (existingStyle) {
      existingStyle.textContent = css;
    } else {
      const style = document.createElement('style');
      style.id = styleId;
      style.textContent = css;
      document.head.appendChild(style);
    }
  }
}

export function updateGlassMode(enabled: boolean) {
  if (typeof document !== 'undefined') {
    if (enabled) {
      document.body.setAttribute('data-glass', 'true');
    } else {
      document.body.removeAttribute('data-glass');
    }
  }
}
