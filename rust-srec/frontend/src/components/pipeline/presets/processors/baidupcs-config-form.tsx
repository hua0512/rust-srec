import { useState } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { useWatch } from 'react-hook-form';
import { toast } from 'sonner';
import { queryKeys } from '@/api/query-keys';
import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';
import { Progress } from '@/components/ui/progress';
import { FlagFormField } from '@/components/ui/flag-form-field';
import { Skeleton } from '@/components/ui/skeleton';
import { type BaiduPcsConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import {
  Cloud,
  CloudUpload,
  Loader2,
  LogIn,
  LogOut,
  RefreshCw,
  Settings2,
  Terminal,
  TriangleAlert,
  UserRound,
} from 'lucide-react';
import {
  ProcessorConfigFormProps,
  useProcessorFieldPath,
} from './common-props';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { FieldHint } from './field-hint';
import { PLACEHOLDER_TOKENS } from '../../constants';
import { formatBytes } from '@/lib/format';
import { getBaiduPcsStatus, baiduPcsLogout } from '@/server/functions/baidupcs';
import { BaiduPcsLoginDialog } from './baidupcs-login-dialog';
import {
  ProcessorListField,
  ProcessorNumberField,
  ProcessorSection,
  ProcessorSelectField,
  ProcessorTextField,
  TimeAnchorField,
} from './processor-form-layout';

type BaiduPcsConfig = z.infer<typeof BaiduPcsConfigSchema>;

const POLICIES = [
  { value: 'skip', label: <Trans>Skip existing files (default)</Trans> },
  { value: 'overwrite', label: <Trans>Overwrite existing files</Trans> },
  { value: 'rsync', label: <Trans>Overwrite only when size changed</Trans> },
];

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
    queryKey: queryKeys.baiduPcs.status(
      overrides.binary_path ?? '',
      overrides.config_dir ?? '',
    ),
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
      void queryClient.invalidateQueries({ queryKey: queryKeys.baiduPcs.all });
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
    <ProcessorSection
      icon={UserRound}
      iconClassName="text-primary"
      title={<Trans>Baidu Netdisk Account</Trans>}
      actions={
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
      }
    >
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

      <BaiduPcsLoginDialog
        open={loginOpen}
        onOpenChange={setLoginOpen}
        binaryPath={overrides.binary_path}
        configDir={overrides.config_dir}
      />
    </ProcessorSection>
  );
}

export function BaiduPcsConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps) {
  const { i18n } = useLingui();
  const fieldPath = useProcessorFieldPath<BaiduPcsConfig>(pathPrefix);

  const binaryPath = useWatch({
    control,
    name: fieldPath('binary_path'),
  }) as string | undefined;
  const configDir = useWatch({
    control,
    name: fieldPath('config_dir'),
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
          <ProcessorSection
            icon={Cloud}
            iconClassName="text-primary"
            title={<Trans>Destination</Trans>}
          >
            <ProcessorTextField
              control={control}
              name={fieldPath('destination_root')}
              label={<Trans>Destination Folder</Trans>}
              hint={
                <FieldHint label={i18n._(msg`Destination Folder help`)}>
                  <p>
                    <Trans>
                      Netdisk folder path. Supports placeholders:{' '}
                      {PLACEHOLDER_TOKENS} and time tokens like %Y/%m/%d.
                    </Trans>
                  </p>
                  <p>
                    <Trans>
                      Missing folders are created by BaiduPCS-Go during upload.
                    </Trans>
                  </p>
                </FieldHint>
              }
              description={
                <Trans>
                  Baidu Netdisk folder the files are uploaded into. Defaults to
                  the netdisk root when empty.
                </Trans>
              }
              placeholder={i18n._(msg`e.g. /rust-srec/{streamer}/%Y-%m`)}
              className="font-mono text-sm"
            />
            <TimeAnchorField
              control={control}
              name={fieldPath('time_anchor')}
              description={
                <Trans>
                  Use stream start time to keep a midnight-crossing stream in
                  one dated folder and group all segments of a session together.
                </Trans>
              }
            />
          </ProcessorSection>

          <ProcessorSection
            icon={CloudUpload}
            iconClassName="text-primary"
            title={<Trans>Upload Behavior</Trans>}
          >
            <ProcessorSelectField
              control={control}
              name={fieldPath('policy')}
              label={<Trans>Same-name policy</Trans>}
              placeholder={i18n._(msg`Select policy`)}
              description={
                <Trans>
                  How files that already exist at the destination are handled
                  (--policy). Skip also makes job retries cheap.
                </Trans>
              }
              options={POLICIES}
            />
            <FlagFormField
              fieldName={fieldPath('norapid')}
              title={<Trans>Disable rapid upload</Trans>}
              description={
                <Trans>
                  Skips the rapid-upload (秒传) hash check before transferring
                  (--norapid).
                </Trans>
              }
            />
            <FlagFormField
              fieldName={fieldPath('remove_source_after_upload')}
              title={<Trans>Delete local files after upload</Trans>}
              className="border-destructive/30 bg-destructive/5"
              description={
                <Trans>
                  Removes each local file once its upload (or skip) is
                  confirmed. The local copy is gone afterwards — keep this off
                  unless Netdisk is the final destination.
                </Trans>
              }
            />
            <ProcessorNumberField
              control={control}
              name={fieldPath('max_retries')}
              label={<Trans>Max Attempts</Trans>}
              description={
                <Trans>
                  Upload attempts per job before failing. Retries only re-send
                  files without a confirmed result.
                </Trans>
              }
              min={1}
              max={10}
              step={1}
              placeholder="3"
            />
          </ProcessorSection>
        </TabsContent>

        <TabsContent value="advanced" className="space-y-4">
          <ProcessorSection
            icon={Settings2}
            iconClassName="text-primary"
            title={<Trans>Tool Location</Trans>}
          >
            <ProcessorTextField
              control={control}
              name={fieldPath('binary_path')}
              label={<Trans>BaiduPCS-Go Executable (Optional)</Trans>}
              description={
                <Trans>
                  Falls back to the BAIDUPCS_PATH environment variable, then
                  PATH lookup.
                </Trans>
              }
              placeholder="BaiduPCS-Go"
              className="font-mono text-sm"
            />
            <ProcessorTextField
              control={control}
              name={fieldPath('config_dir')}
              label={<Trans>Config Directory (Optional)</Trans>}
              hint={
                <FieldHint label={i18n._(msg`Config Directory help`)}>
                  <p>
                    <Trans>
                      Directory holding BaiduPCS-Go's login session
                      (BAIDUPCS_GO_CONFIG_DIR). Logging in via the account card
                      above stores the session in the same directory configured
                      here.
                    </Trans>
                  </p>
                </FieldHint>
              }
              placeholder={i18n._(msg`e.g. /app/config/BaiduPCS-Go`)}
              className="font-mono text-sm"
            />
          </ProcessorSection>

          <ProcessorSection
            icon={Terminal}
            iconClassName="text-primary"
            title={<Trans>Extra Arguments</Trans>}
          >
            <ProcessorListField
              control={control}
              name={fieldPath('args')}
              description={<Trans>Double click to edit items.</Trans>}
              placeholder={i18n._(msg`Add BaiduPCS-Go argument`)}
            />
          </ProcessorSection>
        </TabsContent>
      </Tabs>
    </div>
  );
}
