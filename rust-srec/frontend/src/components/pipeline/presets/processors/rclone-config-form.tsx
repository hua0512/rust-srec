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
} from 'lucide-react';
import { ProcessorConfigFormProps } from './common-props';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';

type RcloneConfig = z.infer<typeof RcloneConfigSchema>;

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
          <CardContent className="grid gap-4 pt-4">
            <FormField
              control={control}
              name={`${prefix}bwlimit` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    <Trans>Bandwidth Limit</Trans>
                  </FormLabel>
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
                      Caps overall transfer bandwidth (rclone --bwlimit). Units
                      are bytes (default base KiB/s). Examples: <code>10M</code>{' '}
                      (both directions), <code>10M:100k</code>{' '}
                      (upload:download), or a timetable such as{' '}
                      <code>08:00,512k 23:00,off</code>.
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
                  <FormLabel>
                    <Trans>Per-File Bandwidth Limit</Trans>
                  </FormLabel>
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

            <div className="grid grid-cols-2 gap-4">
              <FormField
                control={control}
                name={`${prefix}transfers` as any}
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>
                      <Trans>Transfers</Trans>
                    </FormLabel>
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
                    <FormLabel>
                      <Trans>Checkers</Trans>
                    </FormLabel>
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

            <div className="grid grid-cols-2 gap-4">
              <FormField
                control={control}
                name={`${prefix}tpslimit` as any}
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>
                      <Trans>TPS Limit</Trans>
                    </FormLabel>
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
                        Max transactions/second to the remote API (--tpslimit).
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
                    <FormLabel>
                      <Trans>TPS Burst</Trans>
                    </FormLabel>
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

            <div className="grid grid-cols-2 gap-4">
              <FormField
                control={control}
                name={`${prefix}multi_thread_streams` as any}
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>
                      <Trans>Multi-Thread Streams</Trans>
                    </FormLabel>
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
                        Streams per file for multi-thread copy
                        (--multi-thread-streams).
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
                    <FormLabel>
                      <Trans>Multi-Thread Cutoff</Trans>
                    </FormLabel>
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
                        File size at which multi-thread copy kicks in
                        (--multi-thread-cutoff).
                      </Trans>
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
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
