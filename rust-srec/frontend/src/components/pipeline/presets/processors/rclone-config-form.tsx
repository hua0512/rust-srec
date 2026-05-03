import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
  FormDescription,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { type RcloneConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { ListInput } from '@/components/ui/list-input';
import { Card, CardContent } from '@/components/ui/card';
import { CardHeaderWithIcon } from '@/components/ui/card-header-with-icon';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  Cloud,
  Settings2,
  Terminal,
  ArrowRightLeft,
  Copy,
  Move,
  RefreshCw,
  Gauge,
  HelpCircle,
  Activity,
  Layers,
  Timer,
  Split,
} from 'lucide-react';
import { ProcessorConfigFormProps } from './common-props';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';

type RcloneConfig = z.infer<typeof RcloneConfigSchema>;

/**
 * A small "?" icon next to a form label that reveals richer guidance
 * (examples, recommended values, gotchas) on hover/focus. Use alongside
 * a brief always-visible `<FormDescription>` so the inline text stays
 * scannable.
 */
function FieldHint({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          type="button"
          tabIndex={-1}
          aria-label={label}
          className="inline-flex h-4 w-4 items-center justify-center rounded-full text-muted-foreground/60 transition-colors hover:text-primary focus-visible:text-primary focus-visible:outline-none"
        >
          <HelpCircle className="h-3.5 w-3.5" />
        </button>
      </TooltipTrigger>
      <TooltipContent
        side="top"
        className="max-w-xs space-y-1.5 text-xs leading-relaxed"
      >
        {children}
      </TooltipContent>
    </Tooltip>
  );
}

/**
 * Subtle subsection heading inside a Card. Used to group related form
 * fields without spawning extra Cards.
 */
function SectionHeader({
  icon: Icon,
  children,
}: {
  icon: React.ComponentType<{ className?: string }>;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center gap-2 pb-1">
      <Icon className="h-3.5 w-3.5 text-muted-foreground/70" />
      <h4 className="text-xs font-medium uppercase tracking-wider text-muted-foreground/80">
        {children}
      </h4>
    </div>
  );
}

export function RcloneConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps<RcloneConfig>) {
  const { i18n } = useLingui();
  const prefix = pathPrefix ? `${pathPrefix}.` : '';

  // Coerce a numeric `<Input type="number">` change event to `number | undefined`.
  // `valueAsNumber` returns `NaN` for empty input; we map that back to `undefined`
  // so the form value matches the optional schema (Option<u32> on the backend).
  const onNumberChange =
    (cb: (v: number | undefined) => void) =>
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const v = e.target.valueAsNumber;
      cb(Number.isNaN(v) ? undefined : v);
    };

  return (
    <Tabs defaultValue="general" className="w-full">
      <TabsList className="grid w-full grid-cols-2 mb-4 bg-muted/20 p-1">
        <TabsTrigger
          value="general"
          className="data-[state=active]:bg-background data-[state=active]:shadow-sm"
        >
          <Trans>General</Trans>
        </TabsTrigger>
        <TabsTrigger
          value="advanced"
          className="data-[state=active]:bg-background data-[state=active]:shadow-sm"
        >
          <Trans>Advanced</Trans>
        </TabsTrigger>
      </TabsList>

      <TabsContent value="general" className="space-y-4">
        {/* Operation Selection */}
        <Card className="border-border/50 bg-muted/10 shadow-sm">
          <CardHeaderWithIcon
            icon={ArrowRightLeft}
            title={<Trans>Operation Mode</Trans>}
            className="border-b border-border/10 bg-muted/5"
            iconBgClassName="p-1.5 bg-background/50 border border-border/20 shadow-sm"
            iconClassName="h-4 w-4"
          />
          <CardContent className="grid gap-4 pt-4">
            <FormField
              control={control}
              name={`${prefix}operation` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    <Trans>Operation</Trans>
                  </FormLabel>
                  <Select
                    key={field.value ?? 'initial'}
                    onValueChange={field.onChange}
                    defaultValue={field.value ?? 'copy'}
                  >
                    <FormControl>
                      <SelectTrigger className="h-11 bg-background/50">
                        <div className="flex items-center gap-2">
                          <SelectValue
                            placeholder={i18n._(msg`Select operation`)}
                          />
                        </div>
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent>
                      <SelectItem value="copy">
                        <div className="flex items-center gap-2">
                          <Copy className="h-4 w-4 text-green-400" />
                          <span>
                            <Trans>Copy</Trans>
                          </span>
                          <span className="ml-2 text-xs text-muted-foreground/50">
                            <Trans>(Preserve source)</Trans>
                          </span>
                        </div>
                      </SelectItem>
                      <SelectItem value="move">
                        <div className="flex items-center gap-2">
                          <Move className="h-4 w-4 text-orange-400" />
                          <span>
                            <Trans>Move</Trans>
                          </span>
                          <span className="ml-2 text-xs text-muted-foreground/50">
                            <Trans>(Delete source)</Trans>
                          </span>
                        </div>
                      </SelectItem>
                      <SelectItem value="sync">
                        <div className="flex items-center gap-2">
                          <RefreshCw className="h-4 w-4 text-blue-400" />
                          <span>
                            <Trans>Sync</Trans>
                          </span>
                          <span className="ml-2 text-xs text-muted-foreground/50">
                            <Trans>(Mirror source)</Trans>
                          </span>
                        </div>
                      </SelectItem>
                    </SelectContent>
                  </Select>
                  <FormDescription>
                    <Trans>
                      Choose how files are transferred to the remote.
                    </Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </CardContent>
        </Card>

        {/* Target Configuration */}
        <Card className="border-border/50 bg-muted/10 shadow-sm">
          <CardHeaderWithIcon
            icon={Cloud}
            title={<Trans>Target Configuration</Trans>}
            className="border-b border-border/10 bg-muted/5"
            iconBgClassName="p-1.5 bg-background/50 border border-border/20 shadow-sm"
            iconClassName="h-4 w-4"
          />
          <CardContent className="grid gap-4 pt-4">
            <FormField
              control={control}
              name={`${prefix}destination_root` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    <Trans>Destination Root</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      placeholder={i18n._(msg`e.g. gdrive:/videos`)}
                      {...field}
                      value={field.value ?? ''}
                      className="h-11 bg-background/50 font-mono text-sm"
                    />
                  </FormControl>
                  <FormDescription>
                    <Trans>Base path for remote storage.</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            <div className="grid grid-cols-2 gap-4">
              <FormField
                control={control}
                name={`${prefix}config_path` as any}
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>
                      <Trans>Config Path (Optional)</Trans>
                    </FormLabel>
                    <FormControl>
                      <Input
                        placeholder={i18n._(msg`/path/to/rclone.conf`)}
                        {...field}
                        value={field.value ?? ''}
                        className="bg-background/50"
                      />
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />
              <FormField
                control={control}
                name={`${prefix}rclone_path` as any}
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>
                      <Trans>Rclone Executable</Trans>
                    </FormLabel>
                    <FormControl>
                      <Input
                        {...field}
                        value={field.value ?? ''}
                        className="bg-background/50"
                      />
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />
            </div>
          </CardContent>
        </Card>
      </TabsContent>

      <TabsContent value="advanced" className="space-y-4">
        {/* Retry Policy */}
        <Card className="border-border/50 bg-muted/10 shadow-sm">
          <CardHeaderWithIcon
            icon={Settings2}
            title={<Trans>Retry Policy</Trans>}
            className="border-b border-border/10 bg-muted/5"
            iconBgClassName="p-1.5 bg-background/50 border border-border/20 shadow-sm"
            iconClassName="h-4 w-4"
          />
          <CardContent className="grid gap-4 pt-4">
            <FormField
              control={control}
              name={`${prefix}max_retries` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    <Trans>Max Retries</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      type="number"
                      {...field}
                      onChange={(e) => field.onChange(parseInt(e.target.value))}
                      className="bg-background/50"
                    />
                  </FormControl>
                  <FormDescription>
                    <Trans>Number of attempts before failing the upload.</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </CardContent>
        </Card>

        {/* Throughput */}
        <Card className="border-border/50 bg-muted/10 shadow-sm">
          <CardHeaderWithIcon
            icon={Gauge}
            title={<Trans>Throughput</Trans>}
            className="border-b border-border/10 bg-muted/5"
            iconBgClassName="p-1.5 bg-background/50 border border-border/20 shadow-sm"
            iconClassName="h-4 w-4"
          />
          <CardContent className="space-y-6 pt-4">
            {/* Bandwidth */}
            <div className="space-y-3">
              <SectionHeader icon={Activity}>
                <Trans>Bandwidth</Trans>
              </SectionHeader>
              <FormField
                control={control}
                name={`${prefix}bwlimit` as any}
                render={({ field }) => (
                  <FormItem>
                    <div className="flex items-center gap-1.5">
                      <FormLabel>
                        <Trans>Bandwidth Limit</Trans>
                      </FormLabel>
                      <FieldHint label={i18n._(msg`Bandwidth Limit help`)}>
                        <p>
                          <Trans>
                            Units are bytes (default base KiB/s). Use suffixes
                            B, K, M, G, T, P for larger sizes.
                          </Trans>
                        </p>
                        <p>
                          <Trans>Examples:</Trans>
                        </p>
                        <ul className="list-disc space-y-0.5 pl-4">
                          <li>
                            <code>10M</code> —{' '}
                            <Trans>both directions at 10 MiB/s</Trans>
                          </li>
                          <li>
                            <code>10M:100k</code> —{' '}
                            <Trans>upload : download (asymmetric)</Trans>
                          </li>
                          <li>
                            <code>08:00,512k 23:00,off</code> —{' '}
                            <Trans>time-of-day timetable</Trans>
                          </li>
                        </ul>
                        <p className="text-muted-foreground/70">
                          <Trans>
                            Tip: 1 Mbit/s ≈ 0.125M (divide bits by 8).
                          </Trans>
                        </p>
                      </FieldHint>
                    </div>
                    <FormControl>
                      <Input
                        placeholder={i18n._(msg`e.g. 10M`)}
                        {...field}
                        value={field.value ?? ''}
                        className="h-11 bg-background/50 font-mono text-sm"
                      />
                    </FormControl>
                    <FormDescription>
                      <Trans>
                        Caps overall transfer bandwidth (rclone --bwlimit).
                      </Trans>
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
              <FormField
                control={control}
                name={`${prefix}bwlimit_file` as any}
                render={({ field }) => (
                  <FormItem>
                    <div className="flex items-center gap-1.5">
                      <FormLabel>
                        <Trans>Per-File Bandwidth Limit</Trans>
                      </FormLabel>
                      <FieldHint
                        label={i18n._(msg`Per-File Bandwidth Limit help`)}
                      >
                        <p>
                          <Trans>
                            Limits each individual file's transfer rate rather
                            than the overall total.
                          </Trans>
                        </p>
                        <p>
                          <Trans>
                            Composes with Bandwidth Limit — useful to keep one
                            big file from saturating the whole link while still
                            allowing many small ones to run in parallel.
                          </Trans>
                        </p>
                      </FieldHint>
                    </div>
                    <FormControl>
                      <Input
                        placeholder={i18n._(msg`e.g. 1M`)}
                        {...field}
                        value={field.value ?? ''}
                        className="h-11 bg-background/50 font-mono text-sm"
                      />
                    </FormControl>
                    <FormDescription>
                      <Trans>
                        Per-file cap (rclone --bwlimit-file). Same syntax as
                        Bandwidth Limit; composes with it.
                      </Trans>
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
            </div>

            {/* Concurrency */}
            <div className="space-y-3 border-t border-border/30 pt-5">
              <SectionHeader icon={Layers}>
                <Trans>Concurrency</Trans>
              </SectionHeader>
              <div className="grid grid-cols-2 gap-4">
                <FormField
                  control={control}
                  name={`${prefix}transfers` as any}
                  render={({ field }) => (
                    <FormItem>
                      <div className="flex items-center gap-1.5">
                        <FormLabel>
                          <Trans>Transfers</Trans>
                        </FormLabel>
                        <FieldHint label={i18n._(msg`Transfers help`)}>
                          <p>
                            <Trans>
                              How many files rclone uploads in parallel.
                            </Trans>
                          </p>
                          <p>
                            <Trans>
                              Higher = faster on fast connections, but more
                              aggressive on the remote and your CPU. Increase
                              only if your network and provider can handle it.
                            </Trans>
                          </p>
                        </FieldHint>
                      </div>
                      <FormControl>
                        <Input
                          type="number"
                          min={1}
                          step={1}
                          placeholder={i18n._(msg`rclone default: 4`)}
                          value={field.value ?? ''}
                          onChange={onNumberChange(field.onChange)}
                          className="bg-background/50"
                        />
                      </FormControl>
                      <FormDescription>
                        <Trans>Concurrent file transfers (--transfers).</Trans>
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
                <FormField
                  control={control}
                  name={`${prefix}checkers` as any}
                  render={({ field }) => (
                    <FormItem>
                      <div className="flex items-center gap-1.5">
                        <FormLabel>
                          <Trans>Checkers</Trans>
                        </FormLabel>
                        <FieldHint label={i18n._(msg`Checkers help`)}>
                          <p>
                            <Trans>
                              Parallel integrity checks (e.g. comparing source /
                              destination hashes).
                            </Trans>
                          </p>
                          <p>
                            <Trans>
                              Cheap operations, so the default of 8 is usually
                              fine. Increase if you have many small files
                              against a slow remote.
                            </Trans>
                          </p>
                        </FieldHint>
                      </div>
                      <FormControl>
                        <Input
                          type="number"
                          min={1}
                          step={1}
                          placeholder={i18n._(msg`rclone default: 8`)}
                          value={field.value ?? ''}
                          onChange={onNumberChange(field.onChange)}
                          className="bg-background/50"
                        />
                      </FormControl>
                      <FormDescription>
                        <Trans>Concurrent checkers (--checkers).</Trans>
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </div>
            </div>

            {/* Rate Limiting */}
            <div className="space-y-3 border-t border-border/30 pt-5">
              <SectionHeader icon={Timer}>
                <Trans>Rate Limiting</Trans>
              </SectionHeader>
              <div className="grid grid-cols-2 gap-4">
                <FormField
                  control={control}
                  name={`${prefix}tpslimit` as any}
                  render={({ field }) => (
                    <FormItem>
                      <div className="flex items-center gap-1.5">
                        <FormLabel>
                          <Trans>TPS Limit</Trans>
                        </FormLabel>
                        <FieldHint label={i18n._(msg`TPS Limit help`)}>
                          <p>
                            <Trans>
                              Rate-limits API calls to the remote. Useful when a
                              provider returns 429 "too many requests" under
                              heavy load.
                            </Trans>
                          </p>
                          <p>
                            <Trans>
                              <code>0</code> means unlimited (rclone default).
                              Example: set to <code>10</code> if the provider
                              caps you at 10 requests/second.
                            </Trans>
                          </p>
                        </FieldHint>
                      </div>
                      <FormControl>
                        <Input
                          type="number"
                          min={0}
                          step={0.1}
                          placeholder={i18n._(msg`0 = unlimited`)}
                          value={field.value ?? ''}
                          onChange={onNumberChange(field.onChange)}
                          className="bg-background/50"
                        />
                      </FormControl>
                      <FormDescription>
                        <Trans>
                          Max transactions/second to the remote API
                          (--tpslimit).
                        </Trans>
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
                <FormField
                  control={control}
                  name={`${prefix}tpslimit_burst` as any}
                  render={({ field }) => (
                    <FormItem>
                      <div className="flex items-center gap-1.5">
                        <FormLabel>
                          <Trans>TPS Burst</Trans>
                        </FormLabel>
                        <FieldHint label={i18n._(msg`TPS Burst help`)}>
                          <p>
                            <Trans>
                              How many requests can briefly burst above TPS
                              Limit before throttling kicks in.
                            </Trans>
                          </p>
                          <p>
                            <Trans>
                              Default is 1 (no burst). Increase if the provider
                              tolerates short bursts of activity.
                            </Trans>
                          </p>
                        </FieldHint>
                      </div>
                      <FormControl>
                        <Input
                          type="number"
                          min={0}
                          step={1}
                          placeholder={i18n._(msg`rclone default: 1`)}
                          value={field.value ?? ''}
                          onChange={onNumberChange(field.onChange)}
                          className="bg-background/50"
                        />
                      </FormControl>
                      <FormDescription>
                        <Trans>
                          Burst capacity for TPS Limit (--tpslimit-burst).
                        </Trans>
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </div>
            </div>

            {/* Multi-Thread Copy */}
            <div className="space-y-3 border-t border-border/30 pt-5">
              <SectionHeader icon={Split}>
                <Trans>Multi-Thread Copy</Trans>
              </SectionHeader>
              <div className="grid grid-cols-2 gap-4">
                <FormField
                  control={control}
                  name={`${prefix}multi_thread_streams` as any}
                  render={({ field }) => (
                    <FormItem>
                      <div className="flex items-center gap-1.5">
                        <FormLabel>
                          <Trans>Streams</Trans>
                        </FormLabel>
                        <FieldHint
                          label={i18n._(msg`Multi-Thread Streams help`)}
                        >
                          <p>
                            <Trans>
                              Splits a single large file into N parallel chunks
                              for faster upload.
                            </Trans>
                          </p>
                          <p>
                            <Trans>
                              Only kicks in for files above the Cutoff size.
                              Default is 4. Set to 0 to disable.
                            </Trans>
                          </p>
                        </FieldHint>
                      </div>
                      <FormControl>
                        <Input
                          type="number"
                          min={0}
                          step={1}
                          placeholder={i18n._(msg`rclone default: 4`)}
                          value={field.value ?? ''}
                          onChange={onNumberChange(field.onChange)}
                          className="bg-background/50"
                        />
                      </FormControl>
                      <FormDescription>
                        <Trans>
                          Streams per file (--multi-thread-streams).
                        </Trans>
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
                <FormField
                  control={control}
                  name={`${prefix}multi_thread_cutoff` as any}
                  render={({ field }) => (
                    <FormItem>
                      <div className="flex items-center gap-1.5">
                        <FormLabel>
                          <Trans>Cutoff</Trans>
                        </FormLabel>
                        <FieldHint
                          label={i18n._(msg`Multi-Thread Cutoff help`)}
                        >
                          <p>
                            <Trans>
                              Files smaller than this stay single-threaded.
                              Examples: <code>250M</code>, <code>1G</code>.
                            </Trans>
                          </p>
                          <p>
                            <Trans>
                              Multi-thread upload only helps on large files; on
                              small ones the splitting overhead outweighs the
                              benefit.
                            </Trans>
                          </p>
                        </FieldHint>
                      </div>
                      <FormControl>
                        <Input
                          placeholder={i18n._(msg`e.g. 250M`)}
                          {...field}
                          value={field.value ?? ''}
                          className="bg-background/50 font-mono text-sm"
                        />
                      </FormControl>
                      <FormDescription>
                        <Trans>
                          File size threshold (--multi-thread-cutoff).
                        </Trans>
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </div>
            </div>
          </CardContent>
        </Card>

        {/* Arguments */}
        <Card className="border-border/50 bg-muted/10 shadow-sm">
          <CardHeaderWithIcon
            icon={Terminal}
            title={<Trans>Extra Arguments</Trans>}
            className="border-b border-border/10 bg-muted/5"
            iconBgClassName="p-1.5 bg-background/50 border border-border/20 shadow-sm"
            iconClassName="h-4 w-4"
          />
          <CardContent className="pt-4">
            <FormField
              control={control}
              name={`${prefix}args` as any}
              render={({ field }) => (
                <FormItem>
                  <FormControl>
                    <ListInput
                      value={field.value || []}
                      onChange={field.onChange}
                      placeholder={i18n._(msg`Add rclone argument`)}
                    />
                  </FormControl>
                  <FormDescription>
                    <Trans>Double click to edit items.</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </CardContent>
        </Card>
      </TabsContent>
    </Tabs>
  );
}
