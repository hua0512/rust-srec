import { memo, useState } from 'react';
import { UseFormReturn } from 'react-hook-form';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Textarea } from '@/components/ui/textarea';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import {
  Cookie,
  Network,
  RefreshCw,
  Info,
  UserCheck,
  QrCode,
} from 'lucide-react';
import { RetryPolicyForm } from './retry-policy-form';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  getStreamerCredentialSource,
  getPlatformCredentialSource,
  getTemplateCredentialSource,
  refreshStreamerCredentials,
  refreshPlatformCredentials,
  refreshTemplateCredentials,
} from '@/server/functions';
import { Button } from '@/components/ui/button';
import { toast } from 'sonner';
import { Skeleton } from '@/components/ui/skeleton';
import { Badge } from '@/components/ui/badge';
import { cn } from '@/lib/utils';
import { BilibiliQrLoginDialog } from '@/components/credentials/bilibili-qr-login-dialog';
import type { CredentialSaveScope } from '@/server/functions/credentials';

interface NetworkSettingsCardProps {
  form: UseFormReturn<any>;
  paths: {
    cookies: string;
    retryPolicy: string;
  };
  configMode?: 'json' | 'object';
  credentialScope?: CredentialSaveScope;
  credentialPlatformNameHint?: string;
  streamerId?: string;
}

export const NetworkSettingsCard = memo(
  ({
    form,
    paths,
    configMode = 'object',
    credentialScope,
    credentialPlatformNameHint,
    streamerId,
  }: NetworkSettingsCardProps) => {
    const { i18n } = useLingui();
    const queryClient = useQueryClient();
    const [qrDialogOpen, setQrDialogOpen] = useState(false);

    const scope: CredentialSaveScope | null =
      credentialScope ??
      (streamerId ? { type: 'streamer', id: streamerId } : null);

    const credentialSourceQueryKey = scope
      ? [
          'credentials',
          scope.type,
          scope.id,
          'source',
          scope.type === 'template'
            ? (credentialPlatformNameHint ?? null)
            : null,
        ]
      : ['credentials', 'none'];

    const { data: credentialSource, isLoading: isLoadingCredentials } =
      useQuery({
        queryKey: credentialSourceQueryKey,
        queryFn: async () => {
          if (!scope) return null;
          switch (scope.type) {
            case 'streamer':
              return getStreamerCredentialSource({ data: scope.id });
            case 'platform':
              return getPlatformCredentialSource({ data: scope.id });
            case 'template':
              return getTemplateCredentialSource({
                data: { id: scope.id, platform: credentialPlatformNameHint },
              });
          }
        },
        enabled: !!scope,
      });

    // Determine if this is a bilibili platform for QR login
    const platformForQr =
      credentialSource?.platform ?? credentialPlatformNameHint ?? null;
    const isBilibili =
      typeof platformForQr === 'string' &&
      platformForQr.toLowerCase() === 'bilibili';

    const refreshMutation = useMutation({
      mutationFn: async () => {
        if (!scope) {
          throw new Error('Credential scope not available');
        }
        switch (scope.type) {
          case 'streamer':
            return refreshStreamerCredentials({ data: scope.id });
          case 'platform':
            return refreshPlatformCredentials({ data: scope.id });
          case 'template':
            return refreshTemplateCredentials({
              data: { id: scope.id, platform: credentialPlatformNameHint },
            });
        }
      },
      onSuccess: (data) => {
        if (data.refreshed) {
          toast.success(i18n._(msg`Credentials refreshed successfully`));
          void queryClient.invalidateQueries({
            queryKey: credentialSourceQueryKey,
          });
        } else if (data.requires_relogin) {
          toast.error(i18n._(msg`Refresh failed: Manual login required`));
        } else {
          toast.info(i18n._(msg`No refresh needed or not supported`));
        }
      },
      onError: (error: any) => {
        toast.error(
          error.message || i18n._(msg`Failed to refresh credentials`),
        );
      },
    });

    return (
      <div className="grid gap-6">
        {/* Authentication Card */}
        <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
          <CardHeader className="pb-3 px-6 pt-6">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-3">
                <div className="p-2 rounded-lg bg-orange-500/10 text-orange-600 dark:text-orange-400">
                  <Cookie className="w-5 h-5" />
                </div>
                <div>
                  <CardTitle className="text-lg">
                    <Trans>Authentication</Trans>
                  </CardTitle>
                  <CardDescription>
                    <Trans>Manage credentials and session cookies.</Trans>
                  </CardDescription>
                </div>
              </div>
              <div className="flex items-center gap-2">
                {/* QR Login button - only for bilibili */}
                {scope && isBilibili && (
                  <Button
                    variant="outline"
                    size="sm"
                    type="button"
                    className="gap-2 h-9 rounded-xl border-purple-200 hover:border-purple-300 hover:bg-purple-50 dark:border-purple-500/20 dark:hover:bg-purple-500/10"
                    onClick={() => setQrDialogOpen(true)}
                  >
                    <QrCode className="w-3.5 h-3.5" />
                    <Trans>QR Login</Trans>
                  </Button>
                )}
                {/* Refresh button */}
                {scope && (
                  <Button
                    variant="outline"
                    size="sm"
                    type="button"
                    className="gap-2 h-9 rounded-xl border-orange-200 hover:border-orange-300 hover:bg-orange-50 dark:border-orange-500/20 dark:hover:bg-orange-500/10"
                    onClick={() => refreshMutation.mutate()}
                    disabled={refreshMutation.isPending}
                  >
                    <RefreshCw
                      className={cn(
                        'w-3.5 h-3.5',
                        refreshMutation.isPending && 'animate-spin',
                      )}
                    />
                    <Trans>Refresh</Trans>
                  </Button>
                )}
              </div>
            </div>
          </CardHeader>
          <CardContent className="px-6 pb-6 space-y-6">
            <FormField
              control={form.control}
              name={paths.cookies}
              render={({ field }) => (
                <FormItem>
                  <div className="flex items-center justify-between">
                    <FormLabel>
                      <Trans>Cookies</Trans>
                    </FormLabel>
                    {field.value && (
                      <Badge
                        variant="outline"
                        className="text-[10px] py-0 h-4 bg-muted/50 font-mono"
                      >
                        {field.value.length} chars
                      </Badge>
                    )}
                  </div>
                  <FormControl>
                    <Textarea
                      {...field}
                      placeholder="key=value; key2=value2"
                      value={field.value ?? ''}
                      className="font-mono text-sm bg-background/50 focus:bg-background min-h-[120px] resize-y rounded-xl"
                    />
                  </FormControl>
                  <FormDescription className="text-xs">
                    <Trans>
                      HTTP cookies for authentication. These are automatically
                      updated when refreshed.
                    </Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            {scope && (
              <div className="pt-2 border-t border-border/50">
                <div className="flex items-center gap-2 mb-3">
                  <Info className="w-4 h-4 text-muted-foreground" />
                  <h4 className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                    <Trans>Credential Source Status</Trans>
                  </h4>
                </div>

                {isLoadingCredentials ? (
                  <div className="space-y-3">
                    <Skeleton className="h-4 w-3/4" />
                    <Skeleton className="h-4 w-1/2" />
                  </div>
                ) : credentialSource ? (
                  <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                    <div className="flex items-start gap-3 p-3 rounded-xl bg-muted/30 border border-border/50">
                      <div className="mt-0.5 p-1.5 rounded-lg bg-blue-500/10 text-blue-600 dark:text-blue-400">
                        <UserCheck className="w-4 h-4" />
                      </div>
                      <div>
                        <p className="text-[10px] font-medium text-muted-foreground uppercase">
                          <Trans>Effective Scope</Trans>
                        </p>
                        <p className="text-sm font-semibold truncate titlecase">
                          {credentialSource.scope_name}
                        </p>
                        <Badge
                          variant="secondary"
                          className="mt-1 text-[10px] h-4"
                        >
                          {credentialSource.scope_type}
                        </Badge>
                      </div>
                    </div>

                    <div className="flex items-start gap-3 p-3 rounded-xl bg-muted/30 border border-border/50">
                      <div className="mt-0.5 p-1.5 rounded-lg bg-green-500/10 text-green-600 dark:text-green-400">
                        <RefreshCw className="w-4 h-4" />
                      </div>
                      <div>
                        <p className="text-[10px] font-medium text-muted-foreground uppercase">
                          <Trans>Auto-Refresh</Trans>
                        </p>
                        <p className="text-sm font-semibold">
                          {credentialSource.has_refresh_token ? (
                            <span className="text-green-600 dark:text-green-400">
                              <Trans>Supported</Trans>
                            </span>
                          ) : (
                            <span className="text-muted-foreground">
                              <Trans>Cookies Only</Trans>
                            </span>
                          )}
                        </p>
                        <p className="text-[10px] text-muted-foreground mt-0.5 leading-tight">
                          {credentialSource.has_refresh_token ? (
                            <Trans>Background refresh enabled</Trans>
                          ) : (
                            <Trans>Manual update required</Trans>
                          )}
                        </p>
                      </div>
                    </div>
                  </div>
                ) : (
                  <div className="text-sm text-muted-foreground italic p-3 rounded-xl bg-muted/20 border">
                    <Trans>No credentials found.</Trans>
                  </div>
                )}
              </div>
            )}
          </CardContent>
        </Card>

        {/* Retry Policy Card */}
        <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
          <CardHeader className="pb-3">
            <div className="flex items-center gap-3">
              <div className="p-2 rounded-lg bg-green-500/10 text-green-600 dark:text-green-400">
                <Network className="w-5 h-5" />
              </div>
              <CardTitle className="text-lg">
                <Trans>Download Retry Policy</Trans>
              </CardTitle>
            </div>
          </CardHeader>
          <CardContent>
            <RetryPolicyForm
              form={form}
              name={paths.retryPolicy}
              mode={configMode}
            />
          </CardContent>
        </Card>

        {/* Bilibili QR Login Dialog */}
        {scope && (
          <BilibiliQrLoginDialog
            open={qrDialogOpen}
            onOpenChange={setQrDialogOpen}
            scope={scope}
            onSuccess={() => {
              void queryClient.invalidateQueries({
                queryKey: credentialSourceQueryKey,
              });
              // Invalidate the main configuration query based on scope to trigger a refresh
              if (scope) {
                const mainQueryKey =
                  scope.type === 'platform'
                    ? ['config', 'platform', scope.id]
                    : scope.type === 'template'
                      ? ['template', scope.id]
                      : ['streamer', scope.id];
                void queryClient.invalidateQueries({ queryKey: mainQueryKey });
              }
              toast.success(i18n._(msg`Credentials saved successfully`));
            }}
          />
        )}
      </div>
    );
  },
);

NetworkSettingsCard.displayName = 'NetworkSettingsCard';
