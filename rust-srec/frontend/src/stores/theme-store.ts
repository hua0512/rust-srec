import { create } from 'zustand';
import { persist } from 'zustand/middleware';

export type ThemeColor =
  | 'zinc'
  | 'red'
  | 'rose'
  | 'orange'
  | 'green'
  | 'blue'
  | 'yellow'
  | 'violet';
export type ThemeRadius = 0 | 0.3 | 0.5 | 0.625 | 0.75 | 1.0;

interface ThemeState {
  themeColor: ThemeColor;
  radius: ThemeRadius;
  customCss: string;
  isCustomCssEnabled: boolean;

  setThemeColor: (color: ThemeColor) => void;
  setRadius: (radius: ThemeRadius) => void;
  setCustomCss: (css: string) => void;
  setIsCustomCssEnabled: (enabled: boolean) => void;
}

export const useThemeStore = create<ThemeState>()(
  persist(
    (set, get) => ({
      themeColor: 'zinc',
      radius: 0.625,
      customCss: '',
      isCustomCssEnabled: false,

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
    }),
    {
      name: 'app-theme-storage',
      onRehydrateStorage: () => (state) => {
        if (state) {
          updateThemeColor(state.themeColor);
          updateRadius(state.radius);
          updateCustomCss(state.customCss, state.isCustomCssEnabled);
        }
      },
    },
  ),
);

// Side Effects Helpers
function updateThemeColor(color: ThemeColor) {
  if (typeof document !== 'undefined') {
    document.body.setAttribute('data-theme-color', color);
  }
}

function updateRadius(radius: number) {
  if (typeof document !== 'undefined') {
    document.body.style.setProperty('--radius', `${radius}rem`);
  }
}

function updateCustomCss(css: string, enabled: boolean) {
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
