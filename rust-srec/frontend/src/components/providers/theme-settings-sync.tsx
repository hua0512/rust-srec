import * as React from 'react';
import { useShallow } from 'zustand/react/shallow';

import { useTheme } from '@/components/providers/theme-provider';
import { shadcnThemePresets } from '@/utils/shadcn-ui-theme-presets';
import { useThemeSettings } from '@/store/theme-settings';

const MEDIA = '(prefers-color-scheme: dark)';

function clearInlineCssVars(root: HTMLElement) {
  const inlineStyles = root.style;
  for (let i = inlineStyles.length - 1; i >= 0; i--) {
    const property = inlineStyles[i];
    if (property.startsWith('--')) {
      root.style.removeProperty(property);
    }
  }
}

function applyCssVars(root: HTMLElement, vars: Record<string, string>) {
  Object.entries(vars).forEach(([key, value]) => {
    root.style.setProperty(`--${key}`, value);
  });
}

function withSidebarVars(vars: Record<string, string>): Record<string, string> {
  const next = { ...vars };

  const fallbacks: Record<string, string> = {
    sidebar: 'background',
    'sidebar-foreground': 'foreground',
    'sidebar-primary': 'primary',
    'sidebar-primary-foreground': 'primary-foreground',
    'sidebar-accent': 'accent',
    'sidebar-accent-foreground': 'accent-foreground',
    'sidebar-border': 'border',
    'sidebar-ring': 'ring',
  };

  for (const [key, fallbackKey] of Object.entries(fallbacks)) {
    if (next[key] !== undefined) continue;
    const fallbackValue = next[fallbackKey];
    if (fallbackValue !== undefined) {
      next[key] = fallbackValue;
    }
  }

  return next;
}

export function ThemeSettingsSync() {
  const { theme } = useTheme();
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
  const [systemIsDark, setSystemIsDark] = React.useState(false);

  React.useEffect(() => {
    void useThemeSettings.persist.rehydrate();
    setHydrated(true);
  }, []);

  React.useEffect(() => {
    if (theme !== 'system') return;
    if (typeof window === 'undefined') return;

    const mql = window.matchMedia(MEDIA);
    const onChange = (event: MediaQueryListEvent) => {
      setSystemIsDark(event.matches);
    };

    setSystemIsDark(mql.matches);
    mql.addEventListener('change', onChange);
    return () => mql.removeEventListener('change', onChange);
  }, [theme]);

  const isDarkMode = theme === 'dark' || (theme === 'system' && systemIsDark);

  React.useEffect(() => {
    if (!hydrated) return;
    if (typeof document === 'undefined') return;

    const root = document.documentElement;

    clearInlineCssVars(root);

    const baseVars =
      settings.base === 'imported' && settings.importedTheme
        ? isDarkMode
          ? settings.importedTheme.dark
          : settings.importedTheme.light
        : isDarkMode
          ? (shadcnThemePresets[settings.preset] ?? shadcnThemePresets.default)
              .styles.dark
          : (shadcnThemePresets[settings.preset] ?? shadcnThemePresets.default)
              .styles.light;

    applyCssVars(root, withSidebarVars(baseVars));

    if (settings.radius) {
      root.style.setProperty('--radius', settings.radius);
    }

    applyCssVars(root, settings.overrides);
  }, [
    hydrated,
    isDarkMode,
    settings.base,
    settings.importedTheme,
    settings.overrides,
    settings.preset,
    settings.radius,
  ]);

  return null;
}
