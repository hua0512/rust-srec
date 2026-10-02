import { useEffect, useRef, useState } from 'react';
import { Trans } from '@lingui/react/macro';
import { QRCodeSVG } from 'qrcode.react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import {
  generateCredentialLogin,
  pollCredentialLogin,
} from '@/server/functions/credential-profiles';
import type { CredentialLoginTarget } from '@/api/schemas/credential-profiles';

export function CredentialLoginDialog({
  target,
  onClose,
  onSuccess,
}: {
  target: CredentialLoginTarget;
  onClose: () => void;
  onSuccess: (profileId: string) => void;
}) {
  const [attempt, setAttempt] = useState(0);
  const [url, setUrl] = useState<string>();
  const [status, setStatus] = useState('loading');
  const [error, setError] = useState<string>();
  const callbacks = useRef({ onSuccess });
  callbacks.current = { onSuccess };
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
              if (result.profile_id)
                callbacks.current.onSuccess(result.profile_id);
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
      <DialogContent>
        <DialogHeader>
          <DialogTitle>
            <Trans>QR Login</Trans>
          </DialogTitle>
          <DialogDescription>
            <Trans>
              This login updates only the selected profile. It does not change
              your account selection.
            </Trans>
          </DialogDescription>
        </DialogHeader>
        {url && (
          <div className="mx-auto rounded bg-white p-4">
            <QRCodeSVG value={url} size={200} />
          </div>
        )}
        {status === 'loading' && (
          <p>
            <Trans>Generating QR code...</Trans>
          </p>
        )}
        {status === 'not_scanned' && (
          <p>
            <Trans>Scan with Bilibili mobile app</Trans>
          </p>
        )}
        {status === 'scanned' && (
          <p>
            <Trans>Scanned! Please confirm on your phone</Trans>
          </p>
        )}
        {status === 'completed' && (
          <p role="status">
            <Trans>Credentials saved successfully</Trans>
          </p>
        )}
        {status === 'conflict' && (
          <p role="alert">
            <Trans>
              The profile changed or its owner was removed. Reload before
              starting a new login.
            </Trans>
          </p>
        )}
        {status === 'expired' && (
          <p>
            <Trans>QR code expired</Trans>
          </p>
        )}
        {error && <p role="alert">{error}</p>}
        {(status === 'expired' || error) && (
          <Button
            type="button"
            onClick={() => setAttempt((value) => value + 1)}
          >
            <Trans>Generate new QR code</Trans>
          </Button>
        )}
      </DialogContent>
    </Dialog>
  );
}
