import type { ColorTheme } from '@/types/theme-customizer';
import { shadcnThemePresets } from '@/utils/shadcn-ui-theme-presets';

// Shadcn theme presets for the dropdown - convert from shadcnThemePresets
export const colorThemes: ColorTheme[] = Object.entries(shadcnThemePresets).map(
  ([key, preset]) => ({
    name: preset.label || key,
    value: key,
    preset: preset,
  }),
);
