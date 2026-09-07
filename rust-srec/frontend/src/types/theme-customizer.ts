/**
 * Theme shape the customizer works with: every CSS variable is present as a
 * plain string. Distinct from `ThemePreset` in `types/theme.ts`, which models
 * the sparse, partially-specified presets stored with a theme.
 */
export interface ThemeCustomizerPreset {
  label?: string;
  styles: {
    light: Record<string, string>;
    dark: Record<string, string>;
  };
}

export interface ColorTheme {
  name: string;
  value: string;
  preset: ThemeCustomizerPreset;
}

export interface SidebarVariant {
  name: string;
  value: 'sidebar' | 'floating' | 'inset';
  description: string;
}

export interface SidebarCollapsibleOption {
  name: string;
  value: 'offcanvas' | 'icon' | 'none';
  description: string;
}

export interface SidebarSideOption {
  name: string;
  value: 'left' | 'right';
}

export interface RadiusOption {
  name: string;
  value: string;
}

export interface BrandColor {
  name: string;
  cssVar: string;
}

export interface ImportedTheme {
  light: Record<string, string>;
  dark: Record<string, string>;
}
