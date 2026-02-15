import React from 'react';
import { createLazyFileRoute } from '@tanstack/react-router';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { motion } from 'motion/react';
import {
  Layout,
  Palette,
  Settings,
  Upload,
  RotateCcw,
  Monitor,
  Moon,
  Sun,
} from 'lucide-react';

import { ColorPicker } from '@/components/color-picker';
import { ImportModal } from '@/components/layout/themes/import-modal';
import { ThemePresetPicker } from '@/components/layout/themes/theme-preset-picker';
import { useTheme } from '@/components/providers/theme-provider';
import { Button } from '@/components/ui/button';
import { Card } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Separator } from '@/components/ui/separator';
import { useSidebar } from '@/components/ui/sidebar';
import { baseColors, radiusOptions } from '@/config/theme-customizer-constants';
import { colorThemes } from '@/config/theme-data';
import { useSidebarConfig } from '@/hooks/use-sidebar-config';
import { useCircularTransition } from '@/hooks/use-circular-transition';
import { cn } from '@/lib/utils';
import { useThemeSettings } from '@/store/theme-settings';
import type { ImportedTheme } from '@/types/theme-customizer';
import { useShallow } from 'zustand/react/shallow';

export const Route = createLazyFileRoute('/_authed/_dashboard/config/theme')({
  component: ConfigTheme,
});

function ConfigTheme() {
  const { i18n } = useLingui();
  const { theme, setTheme } = useTheme();
  const { startTransition } = useCircularTransition();
  const { config: sidebarConfig, updateConfig: updateSidebarConfig } =
    useSidebarConfig();
  const { toggleSidebar, state: sidebarState, isMobile } = useSidebar();
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
      reset: state.reset,
    })),
  );

  const [importModalOpen, setImportModalOpen] = React.useState(false);

  const handleReset = () => {
    themeSettings.reset();

    updateSidebarConfig({
      variant: 'sidebar',
      collapsible: 'icon',
      side: 'left',
    });
  };

  const handleImport = (themeData: ImportedTheme) => {
    themeSettings.setImportedTheme(themeData);
  };

  return (
    <>
      <div className="flex flex-col xl:flex-row gap-8 min-h-[calc(100vh-8rem)] px-4 md:px-0">
        <motion.div
          initial={{ opacity: 0, x: -20 }}
          animate={{ opacity: 1, x: 0 }}
          transition={{ duration: 0.4 }}
          className="flex-1 space-y-8 max-w-2xl w-full mx-auto"
        >
          <div className="flex items-center justify-between gap-4">
            <div>
              <h2 className="text-xl font-semibold">
                <Trans>Theme</Trans>
              </h2>
              <p className="text-sm text-muted-foreground">
                <Trans>
                  Customize the application theme and sidebar layout.
                </Trans>
              </p>
            </div>
            <Button
              variant="outline"
              onClick={handleReset}
              className="cursor-pointer"
            >
              <RotateCcw className="h-4 w-4 mr-2" />
              <Trans>Reset</Trans>
            </Button>
          </div>

          <div className="grid gap-8">
            <Section
              title={<Trans>Theme Mode</Trans>}
              description={<Trans>Select your preferred color scheme.</Trans>}
              icon={Monitor}
            >
              <div className="grid grid-cols-3 gap-3">
                {[
                  { value: 'light', icon: Sun, label: <Trans>Light</Trans> },
                  { value: 'dark', icon: Moon, label: <Trans>Dark</Trans> },
                  {
                    value: 'system',
                    icon: Monitor,
                    label: <Trans>System</Trans>,
                  },
                ].map(({ value, icon: Icon, label }) => (
                  <div
                    key={value}
                    role="button"
                    onClick={(event) => {
                      if (theme === value) return;
                      startTransition(
                        { x: event.clientX, y: event.clientY },
                        () => {
                          setTheme(value as 'light' | 'dark' | 'system');
                        },
                      );
                    }}
                    className={cn(
                      'flex flex-col items-center justify-between rounded-xl border-2 p-4 cursor-pointer transition-all hover:bg-muted/50',
                      theme === value
                        ? 'border-primary bg-primary/5'
                        : 'border-muted bg-transparent',
                    )}
                  >
                    <Icon
                      className={cn(
                        'w-6 h-6 mb-3',
                        theme === value
                          ? 'text-primary'
                          : 'text-muted-foreground',
                      )}
                    />
                    <span className="text-sm font-medium">{label}</span>
                  </div>
                ))}
              </div>
            </Section>

            <Section
              title={<Trans>Theme Preset</Trans>}
              description={
                <Trans>Pick a preset theme or import a custom one.</Trans>
              }
              icon={Palette}
            >
              <div className="space-y-4">
                <div className="flex items-center justify-between gap-3">
                  <Label className="text-sm font-medium">
                    <Trans>Shadcn UI Theme Presets</Trans>
                  </Label>
                  <div className="flex items-center gap-2">
                    <Button
                      type="button"
                      variant="outline"
                      onClick={() => setImportModalOpen(true)}
                      className="cursor-pointer"
                    >
                      <Upload className="h-4 w-4 mr-2" />
                      <Trans>Import</Trans>
                    </Button>
                    <Button
                      type="button"
                      variant="outline"
                      onClick={() => {
                        const randomTheme =
                          colorThemes[
                            Math.floor(Math.random() * colorThemes.length)
                          ];
                        themeSettings.setImportedTheme(null);
                        themeSettings.setPreset(randomTheme.value);
                      }}
                      className="cursor-pointer"
                    >
                      <Settings className="h-4 w-4 mr-2" />
                      <Trans>Random</Trans>
                    </Button>
                  </div>
                </div>

                <div className="rounded-xl border bg-card p-4">
                  <ThemePresetPicker
                    themes={colorThemes}
                    value={
                      themeSettings.base === 'preset'
                        ? themeSettings.preset
                        : null
                    }
                    onValueChange={(value) => {
                      themeSettings.setImportedTheme(null);
                      themeSettings.setPreset(value);
                    }}
                  />
                </div>

                {themeSettings.base === 'imported' &&
                themeSettings.importedTheme ? (
                  <div className="flex items-center justify-between rounded-lg border bg-muted/30 px-3 py-2">
                    <p className="text-sm text-muted-foreground">
                      <Trans>Custom imported theme is active.</Trans>
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
                      <Trans>Clear</Trans>
                    </Button>
                  </div>
                ) : null}
              </div>
            </Section>

            <Section
              title={<Trans>Brand Colors</Trans>}
              description={<Trans>Fine tune brand and surface colors.</Trans>}
              icon={Palette}
            >
              <div className="grid sm:grid-cols-2 gap-4">
                {baseColors.map((color) => (
                  <ColorPicker
                    key={color.cssVar}
                    label={color.name}
                    cssVar={color.cssVar}
                    value={themeSettings.overrides[color.cssVar.slice(2)] || ''}
                    onChange={themeSettings.setOverride}
                  />
                ))}
              </div>
            </Section>

            <Section
              title={<Trans>Border Radius</Trans>}
              description={<Trans>Adjust the roundness of UI elements.</Trans>}
              icon={Layout}
            >
              <div className="grid grid-cols-3 sm:grid-cols-6 gap-2">
                {radiusOptions.map((option) => (
                  <button
                    key={option.value}
                    type="button"
                    className={cn(
                      'relative cursor-pointer rounded-md p-3 border transition-colors',
                      themeSettings.radius === option.value
                        ? 'border-primary bg-primary/5'
                        : 'border-border hover:border-border/60',
                    )}
                    onClick={() => {
                      themeSettings.setRadius(option.value);
                    }}
                  >
                    <div className="text-center">
                      <div className="text-xs font-medium">{option.name}</div>
                    </div>
                  </button>
                ))}
              </div>
            </Section>

            <Section
              title={<Trans>Sidebar Layout</Trans>}
              description={<Trans>Adjust sidebar style and behavior.</Trans>}
              icon={Layout}
            >
              <div className="space-y-6">
                <div className="space-y-2">
                  <Label className="text-sm font-medium">
                    <Trans>Variant</Trans>
                  </Label>
                  <div className="grid grid-cols-3 gap-3">
                    {['sidebar', 'floating', 'inset'].map((value) => (
                      <div
                        key={value}
                        role="button"
                        onClick={() =>
                          updateSidebarConfig({
                            variant: value as 'sidebar' | 'floating' | 'inset',
                          })
                        }
                        className={cn(
                          'rounded-xl border-2 p-4 cursor-pointer transition-all hover:bg-muted/50 text-center',
                          sidebarConfig.variant === value
                            ? 'border-primary bg-primary/5'
                            : 'border-muted bg-transparent',
                        )}
                      >
                        <div className="text-sm font-medium capitalize">
                          {value === 'sidebar' ? (
                            <Trans>Default</Trans>
                          ) : value === 'floating' ? (
                            <Trans>Floating</Trans>
                          ) : (
                            <Trans>Inset</Trans>
                          )}
                        </div>
                      </div>
                    ))}
                  </div>
                </div>

                <Separator />

                <div className="space-y-2">
                  <Label className="text-sm font-medium">
                    <Trans>Collapsible</Trans>
                  </Label>
                  <div className="grid grid-cols-3 gap-3">
                    {[
                      { value: 'offcanvas', label: <Trans>Off Canvas</Trans> },
                      { value: 'icon', label: <Trans>Icon</Trans> },
                      { value: 'none', label: <Trans>None</Trans> },
                    ].map((option) => (
                      <div
                        key={option.value}
                        role="button"
                        onClick={() => {
                          updateSidebarConfig({
                            collapsible: option.value as
                              | 'offcanvas'
                              | 'icon'
                              | 'none',
                          });
                          if (
                            option.value === 'icon' &&
                            !isMobile &&
                            sidebarState === 'expanded'
                          ) {
                            toggleSidebar();
                          }
                        }}
                        className={cn(
                          'rounded-xl border-2 p-4 cursor-pointer transition-all hover:bg-muted/50 text-center',
                          sidebarConfig.collapsible === option.value
                            ? 'border-primary bg-primary/5'
                            : 'border-muted bg-transparent',
                        )}
                      >
                        <div className="text-sm font-medium">
                          {option.label}
                        </div>
                      </div>
                    ))}
                  </div>
                </div>

                <Separator />

                <div className="space-y-2">
                  <Label className="text-sm font-medium">
                    <Trans>Position</Trans>
                  </Label>
                  <div className="grid grid-cols-2 gap-3">
                    {[
                      { value: 'left', label: <Trans>Left</Trans> },
                      { value: 'right', label: <Trans>Right</Trans> },
                    ].map((option) => (
                      <div
                        key={option.value}
                        role="button"
                        onClick={() =>
                          updateSidebarConfig({
                            side: option.value as 'left' | 'right',
                          })
                        }
                        className={cn(
                          'rounded-xl border-2 p-4 cursor-pointer transition-all hover:bg-muted/50 text-center',
                          sidebarConfig.side === option.value
                            ? 'border-primary bg-primary/5'
                            : 'border-muted bg-transparent',
                        )}
                      >
                        <div className="text-sm font-medium">
                          {option.label}
                        </div>
                      </div>
                    ))}
                  </div>
                </div>
              </div>
            </Section>
          </div>
        </motion.div>

        <motion.div
          initial={{ opacity: 0, x: 20 }}
          animate={{ opacity: 1, x: 0 }}
          transition={{ duration: 0.4, delay: 0.1 }}
          className="flex-1 xl:max-w-md 2xl:max-w-lg hidden lg:block"
        >
          <div className="sticky top-24 space-y-4">
            <div className="flex items-center justify-between">
              <h2 className="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
                <Trans>Live Preview</Trans>
              </h2>
            </div>
            <ThemePreview i18n={i18n} />
          </div>
        </motion.div>
      </div>

      <ImportModal
        open={importModalOpen}
        onOpenChange={setImportModalOpen}
        onImport={handleImport}
      />
    </>
  );
}

function Section({
  title,
  description,
  icon: Icon,
  children,
}: {
  title: React.ReactNode;
  description: React.ReactNode;
  icon: React.ComponentType<{ className?: string }>;
  children: React.ReactNode;
}) {
  return (
    <div className="space-y-4">
      <div className="flex items-start gap-4">
        <div className="p-2 rounded-lg bg-primary/10 text-primary mt-1">
          <Icon className="w-5 h-5" />
        </div>
        <div className="space-y-1">
          <h3 className="font-semibold text-lg">{title}</h3>
          <p className="text-sm text-muted-foreground">{description}</p>
        </div>
      </div>
      <div className="pl-0 sm:pl-[3.25rem] w-full">{children}</div>
    </div>
  );
}

function ThemePreview({
  i18n,
}: {
  i18n: ReturnType<typeof useLingui>['i18n'];
}) {
  return (
    <Card className="overflow-hidden border-2 shadow-xl bg-background/50 backdrop-blur-xl">
      <div className="border-b bg-muted/30 p-4 flex items-center gap-2">
        <div className="flex gap-1.5">
          <div className="w-3 h-3 rounded-full bg-red-500/50" />
          <div className="w-3 h-3 rounded-full bg-amber-500/50" />
          <div className="w-3 h-3 rounded-full bg-emerald-500/50" />
        </div>
        <div className="ml-4 h-6 w-full max-w-[200px] rounded-md bg-background/50 text-[10px] flex items-center px-3 text-muted-foreground font-mono">
          dashboard.rust-srec.local
        </div>
      </div>
      <div className="p-6 space-y-6">
        <div className="grid grid-cols-2 gap-4">
          <Card className="bg-card shadow-sm border p-4 space-y-3">
            <div className="space-y-1">
              <div className="text-xs text-muted-foreground">
                <Trans>Status</Trans>
              </div>
              <div className="text-2xl font-bold">
                <Trans>OK</Trans>
              </div>
            </div>
            <Button size="sm" className="w-full">
              <Trans>Primary</Trans>
            </Button>
          </Card>
          <Card className="bg-card shadow-sm border p-4 space-y-3">
            <div className="space-y-1">
              <div className="text-xs text-muted-foreground">
                <Trans>Action</Trans>
              </div>
              <div className="text-2xl font-bold">
                <Trans>Run</Trans>
              </div>
            </div>
            <Button size="sm" variant="secondary" className="w-full">
              <Trans>Secondary</Trans>
            </Button>
          </Card>
        </div>

        <div className="space-y-2">
          <Label className="text-xs text-muted-foreground">
            <Trans>Input</Trans>
          </Label>
          <Input placeholder={i18n._(msg`Type something...`)} />
        </div>

        <div className="flex gap-2">
          <Button className="w-full">
            <Trans>Primary Action</Trans>
          </Button>
          <Button variant="secondary" className="w-full">
            <Trans>Secondary</Trans>
          </Button>
        </div>
      </div>
    </Card>
  );
}
