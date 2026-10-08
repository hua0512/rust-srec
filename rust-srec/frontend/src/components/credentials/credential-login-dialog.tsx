import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Trans } from '@lingui/react/macro';
import { QRCodeSVG } from 'qrcode.react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import {
  AlertCircle,
  CheckCircle2,
  Clock,
  Loader2,
  QrCode,
  RefreshCw,
  ScanLine,
  Smartphone,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { Callout } from '@/components/shared/callout';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import {
  generateCredentialLogin,
  pollCredentialLogin,
} from '@/server/functions/credential-profiles';
import type { CredentialLoginTarget } from '@/api/schemas/credential-profiles';

export function CredentialLoginDialog({
  target,
  accountLabel,
  platformName,
  onClose,
  onSuccess,
}: {
  target: CredentialLoginTarget;
  /** The label of the account being added or logged in again. */
  accountLabel: string;
  /** The platform whose app scans the code. */
  platformName?: string;
  onClose: () => void;
  onSuccess: () => void;
}) {
  const [attempt, setAttempt] = useState(0);
  const [url, setUrl] = useState<string>();
  const [status, setStatus] = useState('loading');
  const [error, setError] = useState<string>();
  const onSuccessRef = useRef(onSuccess);
  onSuccessRef.current = onSuccess;
  const targetKey = JSON.stringify(target);
  useEffect(() => {
    let canceled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    setUrl(undefined);
    setError(undefined);
    setStatus('loading');
    async function run() {
      try {
        const generated = await generateCredentialLogin({
          data: JSON.parse(targetKey) as CredentialLoginTarget,
        });
        if (canceled) return;
        setUrl(generated.url);
        setStatus('not_scanned');
        const poll = async () => {
          try {
            const result = await pollCredentialLogin({
              data: generated.login_id,
            });
            if (canceled) return;
            setStatus(result.status);
            if (result.status === 'completed') {
              setUrl(undefined);
              onSuccessRef.current();
            } else if (
              result.status === 'expired' ||
              result.status === 'conflict'
            )
              setUrl(undefined);
            else timer = setTimeout(poll, 2000);
          } catch (caught) {
            if (!canceled) {
              setError(
                caught instanceof Error ? caught.message : 'QR login failed',
              );
              setUrl(undefined);
            }
          }
        };
        timer = setTimeout(poll, 1000);
      } catch (caught) {
        if (!canceled)
          setError(
            caught instanceof Error ? caught.message : 'QR login failed',
          );
      }
    }
    void run();
    return () => {
      canceled = true;
      clearTimeout(timer);
    };
  }, [targetKey, attempt]);
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent
        className="border-border/50 outline-none sm:max-w-sm"
        // Nothing here takes input until the code expires, and the close
        // button should not open with a focus ring; the dialog itself holds
        // focus so Escape and Tab still work.
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          (event.target as HTMLElement | null)?.focus();
        }}
      >
        <DialogHeader>
          <DialogTitle>
            {target.type === 'replace' ? (
              <Trans>Log in again to “{accountLabel}”</Trans>
            ) : (
              <Trans>Log in to “{accountLabel}”</Trans>
            )}
          </DialogTitle>
          <DialogDescription>
            {target.type === 'replace' ? (
              <Trans>
                Scanning replaces this account&apos;s saved login. Account
                selections are not changed.
              </Trans>
            ) : (
              <Trans>
                Scanning adds the account. Account selections are not changed.
              </Trans>
            )}
          </DialogDescription>
        </DialogHeader>
        <div className="flex flex-col items-center gap-4 py-2">
          <div className="flex h-[234px] w-[234px] items-center justify-center rounded-2xl border border-border/50 bg-white p-4 shadow-sm">
            {url ? (
              <QRCodeSVG value={url} size={200} />
            ) : (
              <StatusIcon status={error ? 'error' : status} />
            )}
          </div>
          {status === 'loading' && !error && (
            <StatusLine icon={Loader2} spin>
              <Trans>Generating QR code...</Trans>
            </StatusLine>
          )}
          {status === 'not_scanned' && (
            <StatusLine icon={Smartphone}>
              <ScanPrompt platformName={platformName} />
            </StatusLine>
          )}
          {status === 'scanned' && (
            <StatusLine
              icon={ScanLine}
              className="text-blue-600 dark:text-blue-400"
            >
              <Trans>Scanned! Please confirm on your phone</Trans>
            </StatusLine>
          )}
          {status === 'completed' && (
            <StatusLine
              icon={CheckCircle2}
              role="status"
              className="text-green-600 dark:text-green-400"
            >
              <Trans>Credentials saved successfully</Trans>
            </StatusLine>
          )}
          {status === 'expired' && (
            <StatusLine
              icon={Clock}
              className="text-amber-700 dark:text-amber-400"
            >
              <Trans>QR code expired</Trans>
            </StatusLine>
          )}
          {status === 'conflict' && (
            <Callout tone="error" icon={AlertCircle}>
              <Trans>
                The account or the proxy it uses was changed or removed in the
                meantime. Reload the page and try again.
              </Trans>
            </Callout>
          )}
          {error && (
            <Callout tone="error" icon={AlertCircle}>
              {error}
            </Callout>
          )}
          {(status === 'expired' || error) && (
            <Button
              type="button"
              variant="outline"
              className="gap-1.5 rounded-lg"
              onClick={() => setAttempt((value) => value + 1)}
            >
              <RefreshCw className="h-4 w-4" />
              <Trans>Generate new QR code</Trans>
            </Button>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}

function ScanPrompt({ platformName }: { platformName?: string }) {
  switch (platformName?.toLowerCase()) {
    case 'bilibili':
      return <Trans>Scan with Bilibili mobile app</Trans>;
    case 'douyu':
      return <Trans>Scan with the Douyu app</Trans>;
    default:
      return <Trans>Scan with the platform&apos;s mobile app</Trans>;
  }
}

function StatusLine({
  icon: Icon,
  spin = false,
  role,
  className,
  children,
}: {
  icon: LucideIcon;
  spin?: boolean;
  role?: string;
  className?: string;
  children: ReactNode;
}) {
  return (
    <p
      role={role}
      className={cn(
        'flex items-center gap-2 text-sm font-medium text-muted-foreground',
        className,
      )}
    >
      <Icon className={cn('h-4 w-4 shrink-0', spin && 'animate-spin')} />
      {children}
    </p>
  );
}

/** Fills the QR frame while there is no code to show. */
function StatusIcon({ status }: { status: string }) {
  if (status === 'loading')
    return <Loader2 className="h-10 w-10 animate-spin text-neutral-300" />;
  if (status === 'completed')
    return <CheckCircle2 className="h-12 w-12 text-green-500" />;
  return <QrCode className="h-12 w-12 text-neutral-300" />;
}
