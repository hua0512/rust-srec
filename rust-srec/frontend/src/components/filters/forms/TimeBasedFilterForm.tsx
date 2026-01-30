import { useFormContext, useWatch } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Trans } from '@lingui/react/macro';
import { cn } from '@/lib/utils';
import {
  Clock,
  Info,
  ArrowRight,
  Sun,
  Moon,
  ChevronDown,
  Wand2,
  Calendar,
} from 'lucide-react';
import { IconInput } from '@/components/ui/icon-input';
import {
  Tooltip,
  TooltipProvider,
  TooltipTrigger,
  TooltipContent,
} from '@/components/ui/tooltip';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { useMemo } from 'react';

const DAYS = [
  { id: 'Monday', label: 'M', full: 'Monday' },
  { id: 'Tuesday', label: 'T', full: 'Tuesday' },
  { id: 'Wednesday', label: 'W', full: 'Wednesday' },
  { id: 'Thursday', label: 'T', full: 'Thursday' },
  { id: 'Friday', label: 'F', full: 'Friday' },
  { id: 'Saturday', label: 'S', full: 'Saturday' },
  { id: 'Sunday', label: 'S', full: 'Sunday' },
];

const TIME_PRESETS = [
  { label: 'Full Day', start: '00:00:00', end: '23:59:59' },
  { label: 'Prime Time', start: '19:00:00', end: '23:00:00' },
  { label: 'Morning', start: '06:00:00', end: '12:00:00' },
  { label: 'Night', start: '22:00:00', end: '06:00:00' },
];

const DAY_PRESETS = [
  { label: 'All', type: 'all' as const },
  { label: 'Weekdays', type: 'work' as const },
  { label: 'Weekends', type: 'weekend' as const },
];

export function TimeBasedFilterForm() {
  const { control, setValue } = useFormContext();

  const startTime = useWatch({ control, name: 'config.start_time' });
  const endTime = useWatch({ control, name: 'config.end_time' });

  const duration = useMemo(() => {
    if (!startTime || !endTime) return null;

    const [h1, m1, s1] = startTime.split(':').map(Number);
    const [h2, m2, s2] = endTime.split(':').map(Number);

    let diffInSeconds =
      h2 * 3600 + m2 * 60 + (s2 || 0) - (h1 * 3600 + m1 * 60 + (s1 || 0));

    if (diffInSeconds < 0) {
      diffInSeconds += 24 * 3600;
    }

    const hours = Math.floor(diffInSeconds / 3600);
    const minutes = Math.floor((diffInSeconds % 3600) / 60);

    return { hours, minutes };
  }, [startTime, endTime]);

  const setPreset = (
    onChange: (...event: any[]) => void,
    type: 'all' | 'work' | 'weekend',
  ) => {
    switch (type) {
      case 'all':
        onChange(DAYS.map((d) => d.id));
        break;
      case 'work':
        onChange(['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday']);
        break;
      case 'weekend':
        onChange(['Saturday', 'Sunday']);
        break;
    }
  };

  const setTimePreset = (start: string, end: string) => {
    setValue('config.start_time', start);
    setValue('config.end_time', end);
  };

  const isInRange = (hour: number) => {
    if (!startTime || !endTime) return false;
    const startHour = parseInt(startTime.split(':')[0]);
    const endHour = parseInt(endTime.split(':')[0]);

    if (startHour <= endHour) {
      return hour >= startHour && hour <= endHour;
    } else {
      return hour >= startHour || hour <= endHour;
    }
  };

  return (
    <div className="space-y-8 p-4">
      <FormField
        control={control}
        name="config.days_of_week"
        render={({ field }) => (
          <FormItem className="space-y-4">
            <div className="flex justify-between items-end gap-4">
              <div className="flex-1 min-w-0">
                <FormLabel className="text-base font-semibold">
                  <Trans>Active Days</Trans>
                </FormLabel>
                <FormDescription className="line-clamp-1">
                  <Trans>
                    Select the days required for this filter to apply.
                  </Trans>
                </FormDescription>
              </div>
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    variant="outline"
                    size="sm"
                    className="h-8 gap-2 bg-background/50 backdrop-blur-sm border-dashed"
                  >
                    <Calendar className="h-3.5 w-3.5 text-muted-foreground" />
                    <span className="text-xs font-semibold uppercase tracking-wider">
                      <Trans>Presets</Trans>
                    </span>
                    <ChevronDown className="h-3.5 w-3.5 opacity-50" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" className="w-40">
                  {DAY_PRESETS.map((p) => (
                    <DropdownMenuItem
                      key={p.label}
                      onClick={() => setPreset(field.onChange, p.type)}
                    >
                      <Trans>{p.label}</Trans>
                    </DropdownMenuItem>
                  ))}
                </DropdownMenuContent>
              </DropdownMenu>
            </div>

            <TooltipProvider delayDuration={300}>
              <div className="flex gap-2 justify-between">
                {DAYS.map((day) => {
                  const isSelected = field.value?.includes(day.id);
                  return (
                    <Tooltip key={day.id}>
                      <TooltipTrigger asChild>
                        <div
                          className={cn(
                            'h-10 w-10 rounded-full flex items-center justify-center cursor-pointer transition-all border font-medium text-sm',
                            isSelected
                              ? 'bg-primary text-primary-foreground border-primary shadow-lg ring-2 ring-primary/20 ring-offset-2 scale-110'
                              : 'bg-background hover:bg-muted text-muted-foreground border-muted hover:border-muted-foreground/30',
                          )}
                          onClick={() => {
                            const current = field.value || [];
                            const updated = current.includes(day.id)
                              ? current.filter((d: string) => d !== day.id)
                              : [...current, day.id];
                            field.onChange(updated);
                          }}
                        >
                          {day.label}
                        </div>
                      </TooltipTrigger>
                      <TooltipContent side="bottom">
                        <Trans>{day.full}</Trans>
                      </TooltipContent>
                    </Tooltip>
                  );
                })}
              </div>
            </TooltipProvider>
            <FormMessage />
          </FormItem>
        )}
      />

      <div className="space-y-6">
        <div className="flex justify-between items-end gap-4">
          <div className="flex-1 min-w-0">
            <FormLabel className="text-base font-semibold">
              <Trans>Time Range</Trans>
            </FormLabel>
            <FormDescription className="line-clamp-1">
              <Trans>Interactive 24-hour timeline selection.</Trans>
            </FormDescription>
          </div>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="outline"
                size="sm"
                className="h-8 gap-2 bg-background/50 backdrop-blur-sm border-dashed"
              >
                <Wand2 className="h-3.5 w-3.5" />
                <span className="text-xs font-semibold uppercase tracking-wider">
                  <Trans>Presets</Trans>
                </span>
                <ChevronDown className="h-3.5 w-3.5 opacity-50" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-48">
              {TIME_PRESETS.map((p) => (
                <DropdownMenuItem
                  key={p.label}
                  onClick={() => setTimePreset(p.start, p.end)}
                  className="flex justify-between items-center"
                >
                  <span>
                    <Trans>{p.label}</Trans>
                  </span>
                  <span className="text-[10px] text-muted-foreground font-mono">
                    {p.start.slice(0, 5)} - {p.end.slice(0, 5)}
                  </span>
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>

        {/* Visual Timeline */}
        <div className="relative pt-6 pb-2 px-1">
          <div className="flex h-12 w-full gap-[2px] rounded-lg bg-muted/30 p-1 border border-border/50 overflow-hidden shadow-inner">
            {Array.from({ length: 24 }).map((_, i) => {
              const active = isInRange(i);
              const isStart = startTime?.startsWith(
                i.toString().padStart(2, '0'),
              );
              const isEnd = endTime?.startsWith(i.toString().padStart(2, '0'));

              return (
                <TooltipProvider key={i} delayDuration={0}>
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <div
                        className={cn(
                          'flex-1 cursor-pointer transition-all rounded-[2px]',
                          active
                            ? 'bg-primary shadow-sm'
                            : 'bg-muted/40 hover:bg-muted',
                          isStart &&
                            'ring-2 ring-primary ring-offset-2 z-10 rounded-sm',
                          isEnd &&
                            'ring-2 ring-primary ring-offset-2 z-10 rounded-sm',
                        )}
                        onClick={() => {
                          const timeStr = `${i.toString().padStart(2, '0')}:00:00`;
                          // If clicking an already selected start, maybe set as end?
                          // For simplicity, let's say left-click sets start, right-click sets end?
                          // Or just alternate.
                          if (!startTime || (startTime && endTime)) {
                            setValue('config.start_time', timeStr);
                            setValue('config.end_time', '');
                          } else {
                            setValue('config.end_time', timeStr);
                          }
                        }}
                      />
                    </TooltipTrigger>
                    <TooltipContent side="top" className="px-2 py-1">
                      <div className="text-[10px] font-bold">{i}:00</div>
                    </TooltipContent>
                  </Tooltip>
                </TooltipProvider>
              );
            })}
          </div>

          {/* Timeline Icons & Labels */}
          <div className="flex justify-between mt-2 px-1 text-[10px] font-medium text-muted-foreground uppercase tracking-widest opacity-70">
            <div className="flex items-center gap-1.5">
              <Moon className="h-3 w-3" />
              <span>00:00</span>
            </div>
            <div className="flex items-center gap-1.5">
              <Sun className="h-3 w-3" />
              <span>12:00</span>
            </div>
            <div className="flex items-center gap-1.5">
              <Moon className="h-3 w-3" />
              <span>24:00</span>
            </div>
          </div>
        </div>

        <div className="grid grid-cols-[1fr,auto,1fr] gap-4 items-center bg-muted/20 p-4 rounded-xl border border-border/30 backdrop-blur-[2px]">
          <FormField
            control={control}
            name="config.start_time"
            render={({ field }) => (
              <FormItem className="space-y-1.5">
                <FormLabel className="font-medium text-[10px] uppercase tracking-wider text-muted-foreground ml-1">
                  <Trans>Start</Trans>
                </FormLabel>
                <FormControl>
                  <IconInput
                    type="time"
                    step="1"
                    icon={Clock}
                    iconPosition="left"
                    {...field}
                    className="font-mono text-sm h-10 shadow-sm transition-all focus:ring-primary/20 bg-background/50"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />

          <div className="mt-6 flex flex-col items-center min-w-[70px]">
            <ArrowRight className="h-4 w-4 text-primary/40 animate-pulse" />
            {duration && (
              <div className="mt-1 h-0 relative">
                <div className="absolute top-0 left-1/2 -translate-x-1/2 whitespace-nowrap px-2 py-0.5 rounded-full bg-primary/10 text-[10px] font-black text-primary uppercase tracking-tight ring-1 ring-primary/20">
                  {duration.hours}h {duration.minutes}m
                </div>
              </div>
            )}
          </div>

          <FormField
            control={control}
            name="config.end_time"
            render={({ field }) => (
              <FormItem className="space-y-1.5">
                <FormLabel className="font-medium text-[10px] uppercase tracking-wider text-muted-foreground ml-1">
                  <Trans>End</Trans>
                </FormLabel>
                <FormControl>
                  <IconInput
                    type="time"
                    step="1"
                    icon={Clock}
                    iconPosition="left"
                    {...field}
                    className="font-mono text-sm h-10 shadow-sm transition-all focus:ring-primary/20 bg-background/50"
                  />
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />
        </div>
      </div>

      <div className="flex items-start gap-4 rounded-2xl bg-gradient-to-br from-primary/5 to-primary/15 p-5 border border-primary/20 shadow-sm transition-all hover:shadow-md group">
        <div className="bg-primary/10 p-2.5 rounded-xl group-hover:bg-primary/20 transition-colors">
          <Info className="h-5 w-5 text-primary" />
        </div>
        <div className="space-y-1.5 flex-1 p-0.5">
          <p className="text-sm font-bold text-foreground tracking-tight">
            <Trans>Precision Schedule</Trans>
          </p>
          <p className="text-xs text-muted-foreground leading-relaxed font-medium">
            <Trans>
              This filter activates only during the window highlighted on the
              timeline. If the start time is after the end time, the window will
              wrap around midnight automatically.
            </Trans>
          </p>
        </div>
      </div>
    </div>
  );
}
