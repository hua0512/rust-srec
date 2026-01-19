import { Dices, Monitor, Moon, Sun, Upload } from 'lucide-react';
import { ColorPicker } from '@/components/color-picker';
import { useTheme } from '@/components/providers/theme-provider';
import { ThemePresetPicker } from '@/components/layout/themes/theme-preset-picker';
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
} from '@/components/ui/accordion';
import { Button } from '@/components/ui/button';
import { Label } from '@/components/ui/label';
import { Separator } from '@/components/ui/separator';
import { baseColors, radiusOptions } from '@/config/theme-customizer-constants';
import { colorThemes } from '@/config/theme-data';
import { useCircularTransition } from '@/hooks/use-circular-transition';
import { useThemeSettings } from '@/store/theme-settings';
import { useShallow } from 'zustand/react/shallow';

interface ThemeTabProps {
  onOpenImport: () => void;
}

export function ThemeTab({ onOpenImport }: ThemeTabProps) {
  const { theme, setTheme } = useTheme();
  const themeSettings = useThemeSettings(
    useShallow((state) => ({
      base: state.base,
      preset: state.preset,
      radius: state.radius,
      overrides: state.overrides,
      importedTheme: state.importedTheme,
      setPreset: state.setPreset,
      setRadius: state.setRadius,
      setOverride: state.setOverride,
      setImportedTheme: state.setImportedTheme,
    })),
  );

  const { startTransition } = useCircularTransition();

  const handleRandomShadcn = () => {
    // Apply a random shadcn theme
    const randomTheme =
      colorThemes[Math.floor(Math.random() * colorThemes.length)];
    themeSettings.setImportedTheme(null);
    themeSettings.setPreset(randomTheme.value);
  };

  const handleRadiusSelect = (radius: string) => {
    themeSettings.setRadius(radius);
  };

  return (
    <div className="p-4 space-y-6">
      {/* Shadcn UI Theme Presets */}
      <div className="space-y-3">
        <div className="flex items-center justify-between">
          <Label className="text-sm font-medium">Shadcn UI Theme Presets</Label>
          <div className="flex items-center gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={onOpenImport}
              className="cursor-pointer"
            >
              <Upload className="h-3.5 w-3.5 mr-1.5" />
              Import
            </Button>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={handleRandomShadcn}
              className="cursor-pointer"
            >
              <Dices className="h-3.5 w-3.5 mr-1.5" />
              Random
            </Button>
          </div>
        </div>

        <ThemePresetPicker
          themes={colorThemes}
          value={themeSettings.base === 'preset' ? themeSettings.preset : null}
          onValueChange={(value) => {
            themeSettings.setImportedTheme(null);
            themeSettings.setPreset(value);
          }}
          showLabels={false}
        />

        {themeSettings.base === 'imported' && themeSettings.importedTheme ? (
          <div className="flex items-center justify-between rounded-lg border bg-muted/30 px-3 py-2">
            <p className="text-sm text-muted-foreground">
              Custom imported theme is active.
            </p>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => {
                themeSettings.setImportedTheme(null);
              }}
              className="cursor-pointer"
            >
              Clear
            </Button>
          </div>
        ) : null}
      </div>

      <Separator />

      {/* Radius Selection */}
      <div className="space-y-3">
        <Label className="text-sm font-medium">Radius</Label>
        <div className="grid grid-cols-6 gap-2">
          {radiusOptions.map((option) => (
            <button
              key={option.value}
              className={`relative cursor-pointer rounded-md p-3 border transition-colors ${
                themeSettings.radius === option.value
                  ? 'border-primary'
                  : 'border-border hover:border-border/60'
              }`}
              onClick={() => handleRadiusSelect(option.value)}
              type="button"
            >
              <div className="text-center">
                <div className="text-xs font-medium">{option.name}</div>
              </div>
            </button>
          ))}
        </div>
      </div>

      <Separator />

      {/* Mode Section */}
      <div className="space-y-3">
        <Label className="text-sm font-medium">Mode</Label>
        <div className="grid grid-cols-3 gap-2">
          <Button
            variant={theme === 'light' ? 'secondary' : 'outline'}
            size="sm"
            onClick={(event) => {
              if (theme === 'light') return;
              startTransition({ x: event.clientX, y: event.clientY }, () => {
                setTheme('light');
              });
            }}
            className="cursor-pointer"
          >
            <Sun className="h-4 w-4 mr-1" />
            Light
          </Button>
          <Button
            variant={theme === 'dark' ? 'secondary' : 'outline'}
            size="sm"
            onClick={(event) => {
              if (theme === 'dark') return;
              startTransition({ x: event.clientX, y: event.clientY }, () => {
                setTheme('dark');
              });
            }}
            className="cursor-pointer"
          >
            <Moon className="h-4 w-4 mr-1" />
            Dark
          </Button>
          <Button
            variant={theme === 'system' ? 'secondary' : 'outline'}
            size="sm"
            onClick={(event) => {
              if (theme === 'system') return;
              startTransition({ x: event.clientX, y: event.clientY }, () => {
                setTheme('system');
              });
            }}
            className="cursor-pointer"
          >
            <Monitor className="h-4 w-4 mr-1" />
            System
          </Button>
        </div>
      </div>

      <Separator />

      {/* Brand Colors Section */}
      <Accordion
        type="single"
        collapsible
        className="w-full border-b rounded-lg"
      >
        <AccordionItem
          value="brand-colors"
          className="border border-border rounded-lg overflow-hidden"
        >
          <AccordionTrigger className="px-4 py-3 hover:no-underline hover:bg-muted/50 transition-colors">
            <Label className="text-sm font-medium cursor-pointer">
              Brand Colors
            </Label>
          </AccordionTrigger>
          <AccordionContent className="px-4 pb-4 pt-2 space-y-3 border-t border-border bg-muted/20">
            {baseColors.map((color) => (
              <div
                key={color.cssVar}
                className="flex items-center justify-between"
              >
                <ColorPicker
                  label={color.name}
                  cssVar={color.cssVar}
                  value={themeSettings.overrides[color.cssVar.slice(2)] ?? ''}
                  onChange={themeSettings.setOverride}
                />
              </div>
            ))}
          </AccordionContent>
        </AccordionItem>
      </Accordion>
    </div>
  );
}
