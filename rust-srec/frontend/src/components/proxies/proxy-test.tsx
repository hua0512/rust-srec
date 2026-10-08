import { useId, useState } from 'react';
import { useMutation } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import type { MessageDescriptor } from '@lingui/core';
import { CheckCircle2, Gauge, Loader2, XCircle } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { formatPlatformName } from '@/lib/format';
import { cn } from '@/lib/utils';
import { testProxy } from '@/server/functions/proxies';
import {
  PROBE_PLATFORMS,
  type ProbeErrorKind,
  type ProbeOutcome,
  type TestProxy,
} from '@/api/schemas/proxies';

/** What a check connects through; the target is chosen in the panel. */
export type ProxyTestEndpoint = Omit<TestProxy, 'platform' | 'target_url'>;

const CUSTOM = 'custom';

export const PROBE_ERRORS: Record<ProbeErrorKind, MessageDescriptor> = {
  proxy_authentication_required: msg`The proxy rejected the login`,
  connect_failed: msg`Could not connect through the proxy`,
  timeout: msg`No answer within 10 seconds`,
  tls: msg`The secure connection to the site failed`,
  failed: msg`The check failed`,
};

/** One check's outcome: reachable with its status and time, or why not. */
export function ProbeResult({ outcome }: { outcome: ProbeOutcome }) {
  const { i18n } = useLingui();
  const latency = outcome.latency_ms;
  const status = outcome.status;
  if (outcome.ok)
    return (
      <p
        role="status"
        className="flex items-center gap-1.5 text-xs font-medium text-emerald-600 dark:text-emerald-400"
      >
        <CheckCircle2 className="size-3.5 shrink-0" />
        <span>
          <Trans>Reachable</Trans>
          {status != null && <> · HTTP {status}</>} · {latency} ms
        </span>
      </p>
    );
  return (
    <p
      role="status"
      className="flex items-center gap-1.5 text-xs font-medium text-destructive"
    >
      <XCircle className="size-3.5 shrink-0" />
      <span>
        {i18n._(PROBE_ERRORS[outcome.error ?? 'failed'])}
        {status != null && <> · HTTP {status}</>}
        {latency > 0 && <> · {latency} ms</>}
      </span>
    </p>
  );
}

/**
 * Checks a proxy by requesting a platform's home page, or any http(s) URL,
 * through it once. `endpoint` returns what to connect through, or nothing
 * while the proxy is not yet usable.
 */
export function ProxyTestPanel({
  endpoint,
  autoFocus = false,
  className,
}: {
  endpoint: () => ProxyTestEndpoint | undefined;
  autoFocus?: boolean;
  className?: string;
}) {
  const { i18n } = useLingui();
  const targetId = useId();
  const [target, setTarget] = useState<string>('bilibili');
  const [customUrl, setCustomUrl] = useState('');
  const run = useMutation({
    mutationFn: (request: TestProxy) => testProxy({ data: request }),
  });
  const through = endpoint();
  const custom = target === CUSTOM;
  const ready = through !== undefined && (!custom || customUrl.trim() !== '');
  const start = () => {
    if (!through) return;
    run.mutate({
      ...through,
      ...(custom ? { target_url: customUrl.trim() } : { platform: target }),
    });
  };
  return (
    <div className={cn('space-y-2', className)}>
      <Label htmlFor={targetId} className="text-xs text-muted-foreground">
        <Trans>Check that it reaches</Trans>
      </Label>
      <div className="flex flex-wrap items-center gap-2">
        <Select
          value={target}
          onValueChange={(value) => {
            setTarget(value);
            run.reset();
          }}
        >
          <SelectTrigger
            id={targetId}
            className="h-9 w-44 rounded-lg border-border/50 bg-background/50"
            autoFocus={autoFocus}
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent className="rounded-xl">
            {PROBE_PLATFORMS.map((platform) => (
              <SelectItem key={platform} value={platform}>
                {formatPlatformName(platform)}
              </SelectItem>
            ))}
            <SelectSeparator />
            <SelectItem value={CUSTOM}>
              <Trans>Another address…</Trans>
            </SelectItem>
          </SelectContent>
        </Select>
        {custom && (
          <Input
            aria-label={i18n._(msg`Address to check`)}
            className="order-last h-9 basis-full rounded-lg border-border/50 bg-background/50 font-mono text-xs"
            placeholder="https://example.com"
            value={customUrl}
            onChange={(event) => {
              setCustomUrl(event.target.value);
              run.reset();
            }}
          />
        )}
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-9 gap-1.5 rounded-lg"
          disabled={!ready || run.isPending}
          onClick={start}
        >
          {run.isPending ? (
            <Loader2 className="size-3.5 animate-spin" />
          ) : (
            <Gauge className="size-3.5" />
          )}
          {run.isPending ? <Trans>Testing…</Trans> : <Trans>Test</Trans>}
        </Button>
      </div>
      {run.data && <ProbeResult outcome={run.data} />}
      {run.error && (
        <p role="status" className="text-xs font-medium text-destructive">
          {run.error.message}
        </p>
      )}
    </div>
  );
}
