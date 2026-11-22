import { Link } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import {
  AlertCircle,
  AlertTriangle,
  CheckCircle2,
  ExternalLink,
  Loader2,
  RefreshCw,
  Wrench,
  XCircle,
} from 'lucide-react';

import { SettingsCard } from '@/components/config/settings-card';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  checkDesktopTools,
  type DesktopToolStatus,
} from '@/desktop/desktop-settings';
import { errorMessage } from '@/lib/api-error';
import { cn } from '@/lib/utils';

const DESKTOP_TOOLS_QUERY_KEY = ['desktop', 'tools'] as const;

function ToolStatusBadge({ status }: { status: DesktopToolStatus['status'] }) {
  switch (status) {
    case 'available':
      return (
        <Badge
          variant="outline"
          className="border-emerald-500/30 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400"
        >
          <CheckCircle2 className="h-3 w-3" aria-hidden="true" />
          <Trans>Available</Trans>
        </Badge>
      );
    case 'outdated':
      return (
        <Badge
          variant="outline"
          className="border-amber-500/30 bg-amber-500/10 text-amber-600 dark:text-amber-400"
        >
          <AlertTriangle className="h-3 w-3" aria-hidden="true" />
          <Trans>Update recommended</Trans>
        </Badge>
      );
    case 'missing':
      return (
        <Badge variant="secondary">
          <AlertCircle className="h-3 w-3" aria-hidden="true" />
          <Trans>Missing</Trans>
        </Badge>
      );
    case 'error':
      return (
        <Badge variant="destructive">
          <XCircle className="h-3 w-3" aria-hidden="true" />
          <Trans>Error</Trans>
        </Badge>
      );
  }
}

export function ExternalToolsCard() {
  const { i18n } = useLingui();
  const {
    data: tools,
    isLoading,
    isFetching,
    isError,
    error,
    refetch,
  } = useQuery({
    queryKey: DESKTOP_TOOLS_QUERY_KEY,
    queryFn: checkDesktopTools,
    // Each check spawns FFmpeg, FFprobe and Streamlink, so it runs on mount
    // and on Re-check only.
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    retry: false,
  });

  return (
    <SettingsCard
      title={<Trans>External Tools</Trans>}
      description={
        <Trans>
          Check the status and versions of local recording and media tools.
        </Trans>
      }
      icon={Wrench}
      iconColor="text-orange-500"
      iconBgColor="bg-orange-500/10"
      action={
        <div className="flex flex-wrap items-center gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={isFetching}
            onClick={() => void refetch()}
            className="gap-1.5"
          >
            <RefreshCw
              className={cn('h-3.5 w-3.5', isFetching && 'animate-spin')}
              aria-hidden="true"
            />
            <Trans>Re-check</Trans>
          </Button>
          <Button asChild variant="outline" size="sm" className="gap-1.5">
            <Link to="/config/engines">
              <ExternalLink className="h-3.5 w-3.5" aria-hidden="true" />
              <Trans>Configure engines</Trans>
            </Link>
          </Button>
        </div>
      }
    >
      {isLoading ? (
        <div className="flex items-center gap-2 rounded-xl border border-border/50 bg-muted/20 p-4 text-xs text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" />
          <Trans>Checking installed tools…</Trans>
        </div>
      ) : isError ? (
        <div className="flex flex-col items-start justify-between gap-3 rounded-xl border border-destructive/30 bg-destructive/5 p-3.5 sm:flex-row sm:items-center">
          <div className="flex items-start gap-2.5">
            <AlertCircle
              className="mt-0.5 h-4 w-4 shrink-0 text-destructive"
              aria-hidden="true"
            />
            <div className="space-y-0.5">
              <p className="text-sm font-medium text-destructive">
                <Trans>Failed to check external tools</Trans>
              </p>
              <p className="text-xs text-muted-foreground">
                {errorMessage(
                  error,
                  i18n._(msg`Unable to inspect local tool binaries.`),
                )}
              </p>
            </div>
          </div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => void refetch()}
            className="shrink-0 gap-1.5"
          >
            <RefreshCw className="h-3.5 w-3.5" aria-hidden="true" />
            <Trans>Retry</Trans>
          </Button>
        </div>
      ) : tools && tools.length > 0 ? (
        <div className="space-y-3">
          {tools.map((tool) => (
            <div
              key={tool.id}
              className="space-y-2 rounded-xl border border-border/50 bg-muted/20 p-3.5"
            >
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div className="flex min-w-0 flex-wrap items-center gap-2">
                  <span className="text-sm font-medium text-foreground">
                    {tool.name}
                  </span>
                  {tool.version && (
                    <Badge variant="outline" className="font-mono text-[11px]">
                      {tool.version}
                    </Badge>
                  )}
                </div>
                <ToolStatusBadge status={tool.status} />
              </div>

              <code
                title={tool.configuredPath}
                className="block min-w-0 truncate font-mono text-xs text-muted-foreground"
              >
                {tool.configuredPath}
              </code>

              {tool.detail && tool.status !== 'available' && (
                <p
                  className={cn(
                    'text-xs leading-relaxed',
                    tool.status === 'outdated'
                      ? 'text-amber-600 dark:text-amber-400'
                      : tool.status === 'error'
                        ? 'text-destructive'
                        : 'text-muted-foreground',
                  )}
                >
                  {tool.detail}
                </p>
              )}
            </div>
          ))}
        </div>
      ) : (
        <p className="text-xs text-muted-foreground">
          <Trans>No external tools reported.</Trans>
        </p>
      )}
    </SettingsCard>
  );
}
