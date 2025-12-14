import { createFileRoute } from '@tanstack/react-router';
import { motion } from 'motion/react';
import { useTheme } from '../../../../components/theme-provider';
import {
  Check,
  Monitor,
  Moon,
  Sun,
  Palette,
  Type,
  LayoutTemplate,
  RotateCcw,
} from 'lucide-react';
import { cn } from '../../../../lib/utils';
import { Trans } from '@lingui/react/macro';
import { Button } from '../../../../components/ui/button';
import { Label } from '../../../../components/ui/label';
import { Textarea } from '../../../../components/ui/textarea';
import { Switch } from '../../../../components/ui/switch';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '../../../../components/ui/dialog';
import { Slider } from '../../../../components/ui/slider';
import { Card, CardContent } from '../../../../components/ui/card';
import { ThemeColor, useThemeStore } from '../../../../stores/theme-store';
import { useState } from 'react';
import { toast } from 'sonner';

export const Route = createFileRoute('/_authed/_dashboard/config/theme')({
  component: ConfigTheme,
});

function ConfigTheme() {
  return (
    <div className="flex flex-col xl:flex-row gap-8 min-h-[calc(100vh-8rem)]">
      <motion.div
        initial={{ opacity: 0, x: -20 }}
        animate={{ opacity: 1, x: 0 }}
        transition={{ duration: 0.4 }}
        className="flex-1 space-y-8 max-w-2xl"
      >
        <div>
          <h1 className="text-3xl font-bold tracking-tight mb-2">
            <Trans>Appearance</Trans>
          </h1>
          <p className="text-muted-foreground">
            <Trans>Customize the look and feel of your dashboard.</Trans>
          </p>
        </div>

        <div className="grid gap-8">
          {/* Theme Mode */}
          <Section
            title={<Trans>Theme Mode</Trans>}
            description={<Trans>Select your preferred color scheme.</Trans>}
            icon={Monitor}
          >
            <ThemeModeSelector />
          </Section>

          {/* Accent Color */}
          <Section
            title={<Trans>Accent Color</Trans>}
            description={
              <Trans>Primary color for buttons and interactions.</Trans>
            }
            icon={Palette}
          >
            <ThemeColorSelector />
          </Section>

          {/* Radius */}
          <Section
            title={<Trans>Border Radius</Trans>}
            description={<Trans>Adjust the roundness of UI elements.</Trans>}
            icon={LayoutTemplate}
          >
            <ThemeRadiusSelector />
          </Section>

          {/* Custom CSS */}
          <Section
            title={<Trans>Custom CSS</Trans>}
            description={<Trans>Advanced theming options.</Trans>}
            icon={Type}
          >
            <CustomThemeImport />
          </Section>
        </div>
      </motion.div>

      {/* Live Preview Panel */}
      <motion.div
        initial={{ opacity: 0, x: 20 }}
        animate={{ opacity: 1, x: 0 }}
        transition={{ duration: 0.4, delay: 0.1 }}
        className="flex-1 xl:max-w-md 2xl:max-w-lg"
      >
        <div className="sticky top-24 space-y-4">
          <div className="flex items-center justify-between">
            <h2 className="text-sm font-semibold text-muted-foreground uppercase tracking-wider">
              <Trans>Live Preview</Trans>
            </h2>
          </div>
          <ThemePreview />
        </div>
      </motion.div>
    </div>
  );
}

// Reuseable Section Component
function Section({
  title,
  description,
  icon: Icon,
  children,
}: {
  title: React.ReactNode;
  description: React.ReactNode;
  icon: any;
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
      <div className="pl-0 sm:pl-[3.25rem]">{children}</div>
    </div>
  );
}

function ThemePreview() {
  return (
    <Card className="overflow-hidden border-2 shadow-xl bg-background/50 backdrop-blur-xl">
      <div className="border-b bg-muted/30 p-4 flex items-center gap-2">
        <div className="flex gap-1.5">
          <div className="w-3 h-3 rounded-full bg-red-500/50" />
          <div className="w-3 h-3 rounded-full bg-amber-500/50" />
          <div className="w-3 h-3 rounded-full bg-emerald-500/50" />
        </div>
        <div className="ml-4 h-6 w-full max-w-[200px] rounded-md bg-background/50 text-[10px] flex items-center px-3 text-muted-foreground font-mono">
          dashboard.example.com
        </div>
      </div>
      <div className="p-6 space-y-6">
        <div className="space-y-2">
          <div className="h-2 w-1/3 rounded-full bg-primary/20" />
          <div className="h-8 w-3/4 rounded-md bg-muted animate-pulse" />
        </div>

        <div className="grid grid-cols-2 gap-4">
          <Card className="bg-card shadow-sm border p-4 space-y-3">
            <div className="flex items-center justify-between">
              <div className="w-8 h-8 rounded-lg bg-primary/10 flex items-center justify-center text-primary">
                <Monitor className="w-4 h-4" />
              </div>
              <span className="text-xs text-muted-foreground">+24%</span>
            </div>
            <div className="space-y-1">
              <div className="text-2xl font-bold">1,234</div>
              <div className="text-xs text-muted-foreground">
                Active Streams
              </div>
            </div>
          </Card>
          <Card className="bg-card shadow-sm border p-4 space-y-3">
            <div className="flex items-center justify-between">
              <div className="w-8 h-8 rounded-lg bg-secondary flex items-center justify-center text-secondary-foreground">
                <RotateCcw className="w-4 h-4" />
              </div>
            </div>
            <div className="space-y-1">
              <div className="text-2xl font-bold">56</div>
              <div className="text-xs text-muted-foreground">Pending Tasks</div>
            </div>
          </Card>
        </div>

        <div className="space-y-4 p-4 rounded-xl border bg-muted/10">
          <div className="space-y-2">
            <Label>Example Input</Label>
            <input
              className="flex h-9 w-full rounded-md border border-input bg-background px-3 py-1 text-sm shadow-sm transition-colors file:border-0 file:bg-transparent file:text-sm file:font-medium placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
              placeholder="Type something..."
            />
          </div>
          <div className="flex gap-2">
            <Button className="w-full">Primary Action</Button>
            <Button variant="secondary" className="w-full">
              Secondary
            </Button>
          </div>
        </div>
      </div>
    </Card>
  );
}

function ThemeModeSelector() {
  const { theme, setTheme } = useTheme();

  return (
    <div className="grid grid-cols-3 gap-3">
      {[
        { value: 'light', icon: Sun, label: <Trans>Light</Trans> },
        { value: 'dark', icon: Moon, label: <Trans>Dark</Trans> },
        { value: 'system', icon: Monitor, label: <Trans>System</Trans> },
      ].map(({ value, icon: Icon, label }) => (
        <div
          key={value}
          role="button"
          onClick={() => setTheme(value)}
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
              theme === value ? 'text-primary' : 'text-muted-foreground',
            )}
          />
          <span className="text-sm font-medium">{label}</span>
        </div>
      ))}
    </div>
  );
}

function ThemeColorSelector() {
  const { themeColor, setThemeColor } = useThemeStore();
  const colors: { name: ThemeColor; class: string }[] = [
    { name: 'zinc', class: 'bg-zinc-900 dark:bg-zinc-100' },
    { name: 'red', class: 'bg-red-500' },
    { name: 'rose', class: 'bg-rose-500' },
    { name: 'orange', class: 'bg-orange-500' },
    { name: 'green', class: 'bg-green-500' },
    { name: 'blue', class: 'bg-blue-500' },
    { name: 'yellow', class: 'bg-yellow-500' },
    { name: 'violet', class: 'bg-violet-500' },
  ];

  return (
    <div className="flex flex-wrap gap-3">
      {colors.map((color) => (
        <button
          key={color.name}
          onClick={() => setThemeColor(color.name)}
          className={cn(
            'group relative flex items-center justify-center w-10 h-10 rounded-full transition-all hover:scale-110 focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2',
            themeColor === color.name &&
              'ring-2 ring-primary ring-offset-2 scale-110',
          )}
        >
          <span
            className={cn('w-8 h-8 rounded-full shadow-sm', color.class)}
          ></span>
          {themeColor === color.name && (
            <Check className="absolute w-4 h-4 text-white mix-blend-difference" />
          )}
        </button>
      ))}
    </div>
  );
}

function ThemeRadiusSelector() {
  const { radius, setRadius } = useThemeStore();
  const radii = [0, 0.3, 0.5, 0.625, 0.75, 1.0];

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between text-sm">
        <span className="text-muted-foreground space-x-2">
          <span>Sharp</span>
          <span className="text-xs font-mono opacity-50">0px</span>
        </span>
        <span className="font-mono font-medium">{radius}rem</span>
        <span className="text-muted-foreground space-x-2">
          <span className="text-xs font-mono opacity-50">16px</span>
          <span>Rounded</span>
        </span>
      </div>
      <div className="relative pt-2">
        <Slider
          defaultValue={[radius]}
          min={0}
          max={1.0}
          step={0.1}
          // Slider doesn't handle arbitrary steps easily if not linear, but radii are roughly linear.
          // Let's use buttons for precision as before, or a slider that snaps.
          // Actually buttons are better for specific tailwind radii.
          className="hidden" // Hiding slider for now, using buttons below.
        />
        <div className="flex justify-between gap-2 p-1 bg-muted/30 rounded-lg border">
          {radii.map((r) => (
            <button
              key={r}
              onClick={() => setRadius(r as any)}
              className={cn(
                'flex-1 py-1.5 rounded-md text-xs font-medium transition-all',
                radius === r
                  ? 'bg-background text-foreground shadow-sm ring-1 ring-black/5 dark:ring-white/10'
                  : 'text-muted-foreground hover:bg-muted/50 hover:text-foreground',
              )}
            >
              {r}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}

function CustomThemeImport() {
  const { customCss, setCustomCss, isCustomCssEnabled, setIsCustomCssEnabled } =
    useThemeStore();
  const [open, setOpen] = useState(false);
  const [tempCss, setTempCss] = useState(customCss);

  const handleSave = () => {
    setCustomCss(tempCss);
    setOpen(false);
    if (!isCustomCssEnabled) {
      setIsCustomCssEnabled(true);
      toast.success(<Trans>Custom theme applied</Trans>);
    }
  };

  return (
    <Card className="border-dashed shadow-none bg-muted/20">
      <CardContent className="flex items-center justify-between p-4">
        <div className="space-y-1">
          <div className="flex items-center gap-2">
            <span className="font-medium text-sm">
              <Trans>Enable Custom CSS</Trans>
            </span>
            {isCustomCssEnabled && (
              <span className="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium bg-primary/10 text-primary">
                Active
              </span>
            )}
          </div>
          <p className="text-xs text-muted-foreground">
            <Trans>Override system colors with your own CSS variables.</Trans>
          </p>
        </div>
        <div className="flex items-center gap-3">
          <Dialog open={open} onOpenChange={setOpen}>
            <DialogTrigger asChild>
              <Button
                variant="outline"
                size="sm"
                onClick={() => setTempCss(customCss)}
              >
                <Trans>Edit CSS</Trans>
              </Button>
            </DialogTrigger>
            <DialogContent className="sm:max-w-xl">
              <DialogHeader>
                <DialogTitle>
                  <Trans>Edit Custom Theme</Trans>
                </DialogTitle>
                <DialogDescription>
                  <Trans>Paste your exported CSS variables here.</Trans>
                </DialogDescription>
              </DialogHeader>
              <Textarea
                value={tempCss}
                onChange={(e) => setTempCss(e.target.value)}
                className="font-mono text-xs min-h-[300px] bg-muted/30"
                placeholder=":root { --background: ... }"
              />
              <DialogFooter>
                <Button onClick={handleSave}>
                  <Trans>Apply Changes</Trans>
                </Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>
          <Switch
            checked={isCustomCssEnabled}
            onCheckedChange={setIsCustomCssEnabled}
          />
        </div>
      </CardContent>
    </Card>
  );
}
