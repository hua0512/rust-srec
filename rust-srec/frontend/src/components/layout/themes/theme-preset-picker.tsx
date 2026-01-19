import { Check } from 'lucide-react';
import type { ColorTheme } from '@/types/theme-customizer';
import { cn } from '@/lib/utils';

function getVar(
  vars: Record<string, string>,
  key: string,
  fallbackKey?: string,
) {
  return vars[key] ?? (fallbackKey ? vars[fallbackKey] : undefined);
}

function ThemePresetIcon({ theme }: { theme: ColorTheme }) {
  const light = theme.preset.styles.light;
  const dark = theme.preset.styles.dark;

  const swatches = [
    getVar(light, 'primary') ?? 'transparent',
    getVar(light, 'secondary') ?? 'transparent',
    getVar(dark, 'sidebar', 'background') ?? 'transparent',
    getVar(dark, 'sidebar-primary', 'primary') ?? 'transparent',
  ];

  return (
    <span className="grid h-8 w-8 grid-cols-2 grid-rows-2 overflow-hidden rounded-full shadow-sm ring-1 ring-black/5 dark:ring-white/10">
      <span style={{ backgroundColor: swatches[0] }} />
      <span style={{ backgroundColor: swatches[1] }} />
      <span style={{ backgroundColor: swatches[2] }} />
      <span style={{ backgroundColor: swatches[3] }} />
    </span>
  );
}

export function ThemePresetPicker({
  themes,
  value,
  onValueChange,
  showLabels = true,
}: {
  themes: ColorTheme[];
  value: string | null;
  onValueChange: (value: string) => void;
  showLabels?: boolean;
}) {
  return (
    <div className="flex flex-wrap gap-3">
      {themes.map((theme) => {
        const selected = value === theme.value;

        return (
          <button
            key={theme.value}
            type="button"
            onClick={() => onValueChange(theme.value)}
            title={theme.name}
            aria-label={theme.name}
            aria-pressed={selected}
            className={cn(
              'group relative flex items-center justify-center gap-2 rounded-full transition-all hover:scale-110 focus:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2',
              showLabels ? 'h-10 pr-4 pl-1' : 'h-10 w-10',
              selected && 'ring-2 ring-primary ring-offset-2 scale-110',
            )}
          >
            <span className="relative">
              <ThemePresetIcon theme={theme} />
              {selected ? (
                <Check className="absolute left-1/2 top-1/2 h-4 w-4 -translate-x-1/2 -translate-y-1/2 text-white mix-blend-difference pointer-events-none" />
              ) : null}
            </span>
            {showLabels ? (
              <span className="text-sm font-medium">{theme.name}</span>
            ) : null}
          </button>
        );
      })}
    </div>
  );
}
