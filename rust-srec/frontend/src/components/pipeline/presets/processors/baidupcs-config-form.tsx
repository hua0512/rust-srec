import { useState } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { useWatch } from 'react-hook-form';
import { toast } from 'sonner';
import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
  FormDescription,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Progress } from '@/components/ui/progress';
import { Switch } from '@/components/ui/switch';
import { Skeleton } from '@/components/ui/skeleton';
import { type BaiduPcsConfigSchema } from '../processor-schemas';
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
  CloudUpload,
  HelpCircle,
  Loader2,
  LogIn,
  LogOut,
  RefreshCw,
  Settings2,
  Terminal,
  TriangleAlert,
  UserRound,
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
import { PLACEHOLDER_TOKENS } from '../../constants';
import { formatBytes } from '@/lib/format';
import { getBaiduPcsStatus, baiduPcsLogout } from '@/server/functions/baidupcs';
import { BaiduPcsLoginDialog } from './baidupcs-login-dialog';

type BaiduPcsConfig = z.infer<typeof BaiduPcsConfigSchema>;

/**
 * A small "?" icon next to a form label that reveals richer guidance on
 * hover/focus, matching the rclone form's FieldHint.
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
 * Live BaiduPCS-Go binary + login-session status with login/logout actions.
 * Re-probes when the form's binary/config-dir overrides change, since those
 * select which session the upload processor will use.
 */
function BaiduPcsAccountCard({
  binaryPath,
  configDir,
}: {
  binaryPath?: string;
  configDir?: string;
}) {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();
  const [loginOpen, setLoginOpen] = useState(false);

  const overrides = {
    binary_path: binaryPath?.trim() || undefined,
    config_dir: configDir?.trim() || undefined,
  };

  const statusQuery = useQuery({
    queryKey: [
      'baidupcs-status',
      overrides.binary_path ?? '',
      overrides.config_dir ?? '',
    ],
    queryFn: () => getBaiduPcsStatus({ data: overrides }),
    staleTime: 30_000,
    refetchOnWindowFocus: false,
  });

  const logoutMutation = useMutation({
    mutationFn: () => baiduPcsLogout({ data: overrides }),
    onSuccess: (result) => {
      if (result.success) {
        toast.success(i18n._(msg`Logged out of Baidu Netdisk`));
      } else {
        toast.error(result.message || i18n._(msg`Baidu Netdisk logout failed`));
      }
      void queryClient.invalidateQueries({ queryKey: ['baidupcs-status'] });
    },
    onError: (error) => {
      toast.error(
        error instanceof Error
          ? error.message
          : i18n._(msg`Baidu Netdisk logout failed`),
      );
    },
  });

  const status = statusQuery.data;
  const quotaPercent =
    status?.quota_used_bytes != null &&
    status?.quota_total_bytes != null &&
    status.quota_total_bytes > 0
      ? Math.min(
          100,
          (status.quota_used_bytes / status.quota_total_bytes) * 100,
        )
      : null;

  return (
    <Card className="border-border/50 bg-muted/10 shadow-sm">
      <CardHeaderWithIcon
        icon={UserRound}
        title={<Trans>Baidu Netdisk Account</Trans>}
        className="border-b border-border/10 bg-muted/5"
        iconBgClassName="p-1.5 bg-background/50 border border-border/20 shadow-sm"
        iconClassName="h-4 w-4"
      >
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-8 w-8"
          aria-label={i18n._(msg`Refresh status`)}
          onClick={() => statusQuery.refetch()}
          disabled={statusQuery.isFetching}
        >
          <RefreshCw
            className={`h-4 w-4 ${statusQuery.isFetching ? 'animate-spin' : ''}`}
          />
        </Button>
      </CardHeaderWithIcon>
      <CardContent className="space-y-3 pt-4">
        {statusQuery.isPending ? (
          <div className="space-y-2">
            <Skeleton className="h-4 w-2/3" />
            <Skeleton className="h-4 w-1/2" />
          </div>
        ) : statusQuery.isError ? (
          <p className="flex items-start gap-2 text-sm text-destructive">
            <TriangleAlert className="mt-0.5 h-4 w-4 shrink-0" />
            <Trans>Failed to query BaiduPCS-Go status.</Trans>
          </p>
        ) : !status?.binary_ok ? (
          <div className="space-y-2">
            <p className="flex items-start gap-2 text-sm text-destructive">
              <TriangleAlert className="mt-0.5 h-4 w-4 shrink-0" />
              <Trans>
                BaiduPCS-Go binary not available at{' '}
                <code className="rounded bg-muted px-1 font-mono text-xs">
                  {status?.resolved_binary_path}
                </code>
                . Install it or set the binary path in the Advanced tab.
              </Trans>
            </p>
            {status?.detail && (
              <p className="break-all font-mono text-xs text-muted-foreground">
                {status.detail}
              </p>
            )}
          </div>
        ) : (
          <>
            <div className="flex flex-wrap items-center gap-2">
              {status.version && (
                <Badge variant="outline" className="font-mono text-xs">
                  {status.version}
                </Badge>
              )}
              {status.logged_in ? (
                <>
                  <Badge className="bg-emerald-500/15 text-emerald-500 hover:bg-emerald-500/15">
                    <Trans>Logged in</Trans>
                  </Badge>
                  <span className="text-sm font-medium">{status.username}</span>
                  {status.uid != null && (
                    <span className="text-xs text-muted-foreground">
                      UID {status.uid}
                    </span>
                  )}
                </>
              ) : (
                <Badge variant="secondary">
                  <Trans>Not logged in</Trans>
                </Badge>
              )}
              {status.has_stored_credentials && (
                <Badge
                  variant="outline"
                  className="border-sky-500/30 text-sky-500"
                >
                  <Trans>Auto re-login</Trans>
                </Badge>
              )}
            </div>

            {status.logged_in && quotaPercent != null && (
              <div className="space-y-1.5">
                <Progress value={quotaPercent} className="h-2" />
                <p className="text-xs text-muted-foreground">
                  <Trans>
                    {formatBytes(status.quota_used_bytes)} of{' '}
                    {formatBytes(status.quota_total_bytes)} used
                  </Trans>
                </p>
              </div>
            )}

            {!status.logged_in && status.detail && (
              <p className="break-all font-mono text-xs text-muted-foreground">
                {status.detail}
              </p>
            )}

            <div className="flex gap-2 pt-1">
              {status.logged_in ? (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => logoutMutation.mutate()}
                  disabled={logoutMutation.isPending}
                >
                  {logoutMutation.isPending ? (
                    <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  ) : (
                    <LogOut className="mr-2 h-4 w-4" />
                  )}
                  <Trans>Log out</Trans>
                </Button>
              ) : (
                <Button
                  type="button"
                  size="sm"
                  onClick={() => setLoginOpen(true)}
                >
                  <LogIn className="mr-2 h-4 w-4" />
                  <Trans>Log in</Trans>
                </Button>
              )}
            </div>
          </>
        )}
      </CardContent>

      <BaiduPcsLoginDialog
        open={loginOpen}
        onOpenChange={setLoginOpen}
        binaryPath={overrides.binary_path}
        configDir={overrides.config_dir}
      />
    </Card>
  );
}

export function BaiduPcsConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps<BaiduPcsConfig>) {
  const { i18n } = useLingui();
  const prefix = pathPrefix ? `${pathPrefix}.` : '';

  const binaryPath = useWatch({
    control,
    name: `${prefix}binary_path` as any,
  }) as string | undefined;
  const configDir = useWatch({
    control,
    name: `${prefix}config_dir` as any,
  }) as string | undefined;

  return (
    <div className="w-full space-y-4">
      <BaiduPcsAccountCard binaryPath={binaryPath} configDir={configDir} />

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
          <Card className="border-border/50 bg-muted/10 shadow-sm">
            <CardHeaderWithIcon
              icon={Cloud}
              title={<Trans>Destination</Trans>}
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
                    <FormLabel className="inline-flex items-center gap-1.5">
                      <Trans>Destination Folder</Trans>
                      <FieldHint label={i18n._(msg`Destination Folder help`)}>
                        <p>
                          <Trans>
                            Netdisk folder path. Supports placeholders:{' '}
                            {PLACEHOLDER_TOKENS} and time tokens like %Y/%m/%d.
                          </Trans>
                        </p>
                        <p>
                          <Trans>
                            Missing folders are created by BaiduPCS-Go during
                            upload.
                          </Trans>
                        </p>
                      </FieldHint>
                    </FormLabel>
                    <FormControl>
                      <Input
                        placeholder={i18n._(
                          msg`e.g. /rust-srec/{streamer}/%Y-%m`,
                        )}
                        {...field}
                        value={field.value ?? ''}
                        className="h-11 bg-background/50 font-mono text-sm"
                      />
                    </FormControl>
                    <FormDescription>
                      <Trans>
                        Baidu Netdisk folder the files are uploaded into.
                        Defaults to the netdisk root when empty.
                      </Trans>
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />

              <FormField
                control={control}
                name={`${prefix}time_anchor` as any}
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>
                      <Trans>Date placeholder anchor</Trans>
                    </FormLabel>
                    <Select
                      onValueChange={field.onChange}
                      value={field.value ?? 'job_created'}
                    >
                      <FormControl>
                        <SelectTrigger className="h-11 bg-background/50">
                          <SelectValue
                            placeholder={i18n._(msg`Select date anchor`)}
                          />
                        </SelectTrigger>
                      </FormControl>
                      <SelectContent>
                        <SelectItem value="job_created">
                          <Trans>Job creation time (default)</Trans>
                        </SelectItem>
                        <SelectItem value="session_start">
                          <Trans>Stream start time</Trans>
                        </SelectItem>
                      </SelectContent>
                    </Select>
                    <FormDescription>
                      <Trans>
                        Use stream start time to keep a midnight-crossing stream
                        in one dated folder and group all segments of a session
                        together.
                      </Trans>
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
            </CardContent>
          </Card>

          <Card className="border-border/50 bg-muted/10 shadow-sm">
            <CardHeaderWithIcon
              icon={CloudUpload}
              title={<Trans>Upload Behavior</Trans>}
              className="border-b border-border/10 bg-muted/5"
              iconBgClassName="p-1.5 bg-background/50 border border-border/20 shadow-sm"
              iconClassName="h-4 w-4"
            />
            <CardContent className="grid gap-4 pt-4">
              <FormField
                control={control}
                name={`${prefix}policy` as any}
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>
                      <Trans>Same-name policy</Trans>
                    </FormLabel>
                    <Select
                      onValueChange={field.onChange}
                      value={field.value ?? 'skip'}
                    >
                      <FormControl>
                        <SelectTrigger className="h-11 bg-background/50">
                          <SelectValue
                            placeholder={i18n._(msg`Select policy`)}
                          />
                        </SelectTrigger>
                      </FormControl>
                      <SelectContent>
                        <SelectItem value="skip">
                          <Trans>Skip existing files (default)</Trans>
                        </SelectItem>
                        <SelectItem value="overwrite">
                          <Trans>Overwrite existing files</Trans>
                        </SelectItem>
                        <SelectItem value="rsync">
                          <Trans>Overwrite only when size changed</Trans>
                        </SelectItem>
                      </SelectContent>
                    </Select>
                    <FormDescription>
                      <Trans>
                        How files that already exist at the destination are
                        handled (--policy). Skip also makes job retries cheap.
                      </Trans>
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />

              <FormField
                control={control}
                name={`${prefix}norapid` as any}
                render={({ field }) => (
                  <FormItem className="flex flex-row items-center justify-between rounded-lg border border-border/40 bg-background/30 p-3">
                    <div className="space-y-0.5 pr-4">
                      <FormLabel>
                        <Trans>Disable rapid upload</Trans>
                      </FormLabel>
                      <FormDescription>
                        <Trans>
                          Skips the rapid-upload (秒传) hash check before
                          transferring (--norapid).
                        </Trans>
                      </FormDescription>
                    </div>
                    <FormControl>
                      <Switch
                        checked={field.value ?? false}
                        onCheckedChange={field.onChange}
                      />
                    </FormControl>
                  </FormItem>
                )}
              />

              <FormField
                control={control}
                name={`${prefix}remove_source_after_upload` as any}
                render={({ field }) => (
                  <FormItem className="flex flex-row items-center justify-between rounded-lg border border-destructive/30 bg-destructive/5 p-3">
                    <div className="space-y-0.5 pr-4">
                      <FormLabel>
                        <Trans>Delete local files after upload</Trans>
                      </FormLabel>
                      <FormDescription>
                        <Trans>
                          Removes each local file once its upload (or skip) is
                          confirmed. The local copy is gone afterwards — keep
                          this off unless Netdisk is the final destination.
                        </Trans>
                      </FormDescription>
                    </div>
                    <FormControl>
                      <Switch
                        checked={field.value ?? false}
                        onCheckedChange={field.onChange}
                      />
                    </FormControl>
                  </FormItem>
                )}
              />

              <FormField
                control={control}
                name={`${prefix}max_retries` as any}
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>
                      <Trans>Max Attempts</Trans>
                    </FormLabel>
                    <FormControl>
                      <Input
                        type="number"
                        min={1}
                        max={10}
                        step={1}
                        {...field}
                        value={field.value ?? 3}
                        onChange={(e) =>
                          field.onChange(parseInt(e.target.value))
                        }
                        className="bg-background/50"
                      />
                    </FormControl>
                    <FormDescription>
                      <Trans>
                        Upload attempts per job before failing. Retries only
                        re-send files without a confirmed result.
                      </Trans>
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="advanced" className="space-y-4">
          <Card className="border-border/50 bg-muted/10 shadow-sm">
            <CardHeaderWithIcon
              icon={Settings2}
              title={<Trans>Tool Location</Trans>}
              className="border-b border-border/10 bg-muted/5"
              iconBgClassName="p-1.5 bg-background/50 border border-border/20 shadow-sm"
              iconClassName="h-4 w-4"
            />
            <CardContent className="grid gap-4 pt-4">
              <FormField
                control={control}
                name={`${prefix}binary_path` as any}
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>
                      <Trans>BaiduPCS-Go Executable (Optional)</Trans>
                    </FormLabel>
                    <FormControl>
                      <Input
                        placeholder={i18n._(msg`BaiduPCS-Go`)}
                        {...field}
                        value={field.value ?? ''}
                        className="bg-background/50 font-mono text-sm"
                      />
                    </FormControl>
                    <FormDescription>
                      <Trans>
                        Falls back to the BAIDUPCS_PATH environment variable,
                        then PATH lookup.
                      </Trans>
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
              <FormField
                control={control}
                name={`${prefix}config_dir` as any}
                render={({ field }) => (
                  <FormItem>
                    <FormLabel className="inline-flex items-center gap-1.5">
                      <Trans>Config Directory (Optional)</Trans>
                      <FieldHint label={i18n._(msg`Config Directory help`)}>
                        <p>
                          <Trans>
                            Directory holding BaiduPCS-Go's login session
                            (BAIDUPCS_GO_CONFIG_DIR). Logging in via the account
                            card above stores the session in the same directory
                            configured here.
                          </Trans>
                        </p>
                      </FieldHint>
                    </FormLabel>
                    <FormControl>
                      <Input
                        placeholder={i18n._(msg`e.g. /app/config/BaiduPCS-Go`)}
                        {...field}
                        value={field.value ?? ''}
                        className="bg-background/50 font-mono text-sm"
                      />
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />
            </CardContent>
          </Card>

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
                        placeholder={i18n._(msg`Add BaiduPCS-Go argument`)}
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
    </div>
  );
}
