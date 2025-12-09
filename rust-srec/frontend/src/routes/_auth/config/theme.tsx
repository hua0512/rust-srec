import { createFileRoute } from '@tanstack/react-router';
import { useTheme } from '../../../components/theme-provider';
import { Check, Monitor, Moon, Sun, Info } from 'lucide-react';
import { flushSync } from 'react-dom';
import { cn } from '../../../lib/utils';
import { Trans } from '@lingui/react/macro';
import { ThemeColor, useThemeColor } from '../../../hooks/use-theme-color';
import { useThemeRadius } from '../../../hooks/use-theme-radius';
import { Button } from '../../../components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '../../../components/ui/card';
import { Label } from '../../../components/ui/label';
import { Textarea } from '../../../components/ui/textarea';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '../../../components/ui/dialog';
import { useCustomTheme } from '../../../hooks/use-custom-theme';
import { useState } from 'react';
import { Switch } from '../../../components/ui/switch';

export const Route = createFileRoute('/_auth/config/theme')({
  component: ConfigTheme,
});

function ConfigTheme() {
  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-lg font-medium">
          <Trans>Appearance</Trans>
        </h3>
        <p className="text-sm text-muted-foreground">
          <Trans>Customize the look and feel of the application.</Trans>
        </p>
      </div>

      <div className="grid gap-6">
        <Card>
          <CardHeader>
            <CardTitle><Trans>Theme Mode</Trans></CardTitle>
            <CardDescription><Trans>Select the color mode for the dashboard.</Trans></CardDescription>
          </CardHeader>
          <CardContent>
            <ThemeSelector />
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle><Trans>Accent Color</Trans></CardTitle>
            <CardDescription><Trans>Choose the primary color for buttons and active elements.</Trans></CardDescription>
          </CardHeader>
          <CardContent>
            <div className="space-y-4">
              <ThemeColorSelector />
              <CustomThemeImport />
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle><Trans>Radius</Trans></CardTitle>
            <CardDescription><Trans>Adjust the roundness of cards and inputs.</Trans></CardDescription>
          </CardHeader>
          <CardContent>
            <ThemeRadiusSelector />
          </CardContent>
        </Card>
      </div>
    </div>
  );
}

function CustomThemeImport() {
  const { customCss, setCustomCss, isEnabled, setIsEnabled } = useCustomTheme();
  const [open, setOpen] = useState(false);
  const [tempCss, setTempCss] = useState(customCss);

  const handleSave = () => {
    setCustomCss(tempCss);
    setIsEnabled(true);
    setOpen(false);
  };

  return (
    <div className="flex flex-col gap-4 rounded-lg border p-4">
      <div className="flex items-center justify-between">
        <div className="space-y-0.5">
          <Label className="text-base"><Trans>Custom Theme</Trans></Label>
          <div className="text-sm text-muted-foreground">
            <Trans>Override styles with a custom CSS theme.</Trans>
          </div>
        </div>
        <Switch
          checked={isEnabled}
          onCheckedChange={setIsEnabled}
        />
      </div>

      {isEnabled && (
        <div className="flex items-center gap-2 rounded-md bg-muted px-3 py-2 text-sm text-muted-foreground">
          <Info className="h-4 w-4" />
          <Trans>Custom theme is active. Native accent colors may be overridden.</Trans>
        </div>
      )}

      <div>
        <Dialog open={open} onOpenChange={setOpen}>
          <DialogTrigger asChild>
            <Button variant="outline" size="sm" onClick={() => setTempCss(customCss)}>
              <Trans>Import Theme</Trans>
            </Button>
          </DialogTrigger>
          <DialogContent className="sm:max-w-[625px]">
            <DialogHeader>
              <DialogTitle><Trans>Import Custom Theme</Trans></DialogTitle>
              <DialogDescription>
                <Trans>Paste the CSS code exported from <strong>tweakcn</strong> or <strong>shadcn/ui</strong> themes.</Trans>
                <div className="mt-2 rounded bg-muted p-2 text-xs font-mono">
                  :root {'{'} <br />
                  &nbsp;&nbsp;--background: 0 0% 100%; <br />
                  &nbsp;&nbsp;--foreground: 222.2 84% 4.9%; <br />
                  &nbsp;&nbsp;/* ... */ <br />
                  {'}'}
                </div>
              </DialogDescription>
            </DialogHeader>
            <div className="grid gap-4 py-4">
              <Textarea
                placeholder=":root { ... }"
                className="h-[300px] font-mono text-xs"
                value={tempCss}
                onChange={(e) => setTempCss(e.target.value)}
              />
            </div>
            <DialogFooter>
              <Button onClick={handleSave}><Trans>Save & Apply</Trans></Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </div>
    </div>
  );
}

function ThemeRadiusSelector() {
  const { radius, setRadius } = useThemeRadius();

  const radii = [0, 0.3, 0.5, 0.625, 0.75, 1.0];

  return (
    <div className="flex flex-wrap gap-2">
      {radii.map((r) => (
        <Button
          key={r}
          variant="outline"
          size="sm"
          onClick={() => setRadius(r)}
          className={cn(
            "w-16",
            radius === r && 'border-2 border-primary bg-primary/10'
          )}
        >
          {r}rem
        </Button>
      ))}
    </div>
  );
}

function ThemeSelector() {
  const { theme, setTheme } = useTheme();

  const handleThemeChange = (newTheme: 'light' | 'dark' | 'system', e: React.MouseEvent) => {
    if (theme === newTheme) return;

    if (
      !document.startViewTransition ||
      window.matchMedia('(prefers-reduced-motion: reduce)').matches
    ) {
      setTheme(newTheme);
      return;
    }

    const x = e.clientX;
    const y = e.clientY;

    const endRadius = Math.hypot(
      Math.max(x, innerWidth - x),
      Math.max(y, innerHeight - y)
    );

    const transition = document.startViewTransition(() => {
      flushSync(() => {
        setTheme(newTheme);
      });
    });

    transition.ready.then(() => {
      const clipPath = [
        `circle(0px at ${x}px ${y}px)`,
        `circle(${endRadius}px at ${x}px ${y}px)`,
      ];

      const isSystemDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
      const isTargetDark = newTheme === 'dark' || (newTheme === 'system' && isSystemDark);

      document.documentElement.animate(
        {
          clipPath: isTargetDark ? clipPath : [...clipPath].reverse(),
        },
        {
          duration: 400,
          easing: 'ease-in',
          pseudoElement: isTargetDark ? '::view-transition-new(root)' : '::view-transition-old(root)',
          fill: 'forwards',
        }
      );
    });
  };

  return (
    <div className="grid max-w-3xl grid-cols-1 gap-4 sm:grid-cols-3">
      <ThemeCard
        active={theme === 'light'}
        onClick={(e) => handleThemeChange('light', e)}
        icon={<Sun className="h-4 w-4" />}
        label={<Trans>Light</Trans>}
      >
        <div className="space-y-2 rounded-sm bg-[#ecedef] p-2">
          <div className="space-y-2 rounded-md bg-white p-2 shadow-sm">
            <div className="h-2 w-[80px] rounded-lg bg-[#ecedef]" />
            <div className="h-2 w-[100px] rounded-lg bg-[#ecedef]" />
          </div>
          <div className="flex items-center space-x-2 rounded-md bg-white p-2 shadow-sm">
            <div className="h-4 w-4 rounded-full bg-[#ecedef]" />
            <div className="h-2 w-[100px] rounded-lg bg-[#ecedef]" />
          </div>
          <div className="flex items-center space-x-2 rounded-md bg-white p-2 shadow-sm">
            <div className="h-4 w-4 rounded-full bg-[#ecedef]" />
            <div className="h-2 w-[100px] rounded-lg bg-[#ecedef]" />
          </div>
        </div>
      </ThemeCard>

      <ThemeCard
        active={theme === 'dark'}
        onClick={(e) => handleThemeChange('dark', e)}
        icon={<Moon className="h-4 w-4" />}
        label={<Trans>Dark</Trans>}
      >
        <div className="space-y-2 rounded-sm bg-slate-950 p-2">
          <div className="space-y-2 rounded-md bg-slate-800 p-2 shadow-sm">
            <div className="h-2 w-[80px] rounded-lg bg-slate-400" />
            <div className="h-2 w-[100px] rounded-lg bg-slate-400" />
          </div>
          <div className="flex items-center space-x-2 rounded-md bg-slate-800 p-2 shadow-sm">
            <div className="h-4 w-4 rounded-full bg-slate-400" />
            <div className="h-2 w-[100px] rounded-lg bg-slate-400" />
          </div>
          <div className="flex items-center space-x-2 rounded-md bg-slate-800 p-2 shadow-sm">
            <div className="h-4 w-4 rounded-full bg-slate-400" />
            <div className="h-2 w-[100px] rounded-lg bg-slate-400" />
          </div>
        </div>
      </ThemeCard>

      <ThemeCard
        active={theme === 'system'}
        onClick={(e) => handleThemeChange('system', e)}
        icon={<Monitor className="h-4 w-4" />}
        label={<Trans>System</Trans>}
      >
        <div className="space-y-2 rounded-sm bg-slate-950 p-2">
          <div className="space-y-2 rounded-md bg-slate-800 p-2 shadow-sm">
            <div className="h-2 w-[80px] rounded-lg bg-slate-400" />
            <div className="h-2 w-[100px] rounded-lg bg-slate-400" />
          </div>
          <div className="flex items-center space-x-2 rounded-md bg-white p-2 shadow-sm">
            <div className="h-4 w-4 rounded-full bg-[#ecedef]" />
            <div className="h-2 w-[100px] rounded-lg bg-[#ecedef]" />
          </div>
          <div className="flex items-center space-x-2 rounded-md bg-white p-2 shadow-sm">
            <div className="h-4 w-4 rounded-full bg-[#ecedef]" />
            <div className="h-2 w-[100px] rounded-lg bg-[#ecedef]" />
          </div>
        </div>
      </ThemeCard>
    </div>
  );
}

function ThemeColorSelector() {
  const { themeColor, setThemeColor } = useThemeColor();
  const { isEnabled } = useCustomTheme();

  const colors: { name: ThemeColor; label: string; class: string }[] = [
    { name: 'zinc', label: 'Zinc', class: 'bg-zinc-900' },
    { name: 'red', label: 'Red', class: 'bg-red-500' },
    { name: 'rose', label: 'Rose', class: 'bg-rose-500' },
    { name: 'orange', label: 'Orange', class: 'bg-orange-500' },
    { name: 'green', label: 'Green', class: 'bg-green-500' },
    { name: 'blue', label: 'Blue', class: 'bg-blue-500' },
    { name: 'yellow', label: 'Yellow', class: 'bg-yellow-500' },
    { name: 'violet', label: 'Violet', class: 'bg-violet-500' },
  ];

  return (
    <div className={cn("grid grid-cols-2 gap-4 sm:grid-cols-4 md:grid-cols-8", isEnabled && "opacity-50 pointer-events-none")}>
      {colors.map((color) => (
        <button
          key={color.name}
          onClick={() => setThemeColor(color.name)}
          className={cn(
            'group flex w-full items-center justify-start space-x-2 rounded-md border p-2 text-left text-sm hover:bg-accent hover:text-accent-foreground',
            themeColor === color.name && 'bg-accent text-accent-foreground'
          )}
        >
          <span className={cn('h-4 w-4 rounded-full', color.class)} />
          <span className="capitalize"><Trans>{color.label}</Trans></span>
          {themeColor === color.name && <Check className="ml-auto h-4 w-4" />}
        </button>
      ))}
    </div>
  );
}

interface ThemeCardProps {
  active: boolean;
  onClick: (e: React.MouseEvent) => void;
  icon: React.ReactNode;
  label: React.ReactNode;
  children: React.ReactNode;
}

function ThemeCard({ active, onClick, icon, label, children }: ThemeCardProps) {
  return (
    <div onClick={onClick} className="cursor-pointer space-y-2" role="button" tabIndex={0}>
      <div
        className={cn(
          'items-center rounded-md border-2 border-muted p-1 hover:border-accent',
          active ? 'border-primary hover:border-primary' : ''
        )}
      >
        <div className="space-y-2 rounded-sm bg-[#ecedef] p-2 dark:bg-slate-950">
          {children}
        </div>
      </div>
      <div className="flex items-center justify-center space-x-2">
        <div
          className={cn(
            'flex items-center justify-center rounded-full p-1',
            active ? 'bg-primary text-primary-foreground text-primary-content' : 'bg-transparent'
          )}
        >
          {active && <Check className="h-3 w-3" />}
        </div>
        <span className="flex items-center gap-2 text-sm font-medium">
          {icon} {label}
        </span>
      </div>
    </div>
  );
}
