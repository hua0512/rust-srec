import { useState, useEffect, useCallback } from 'react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { QRCodeSVG } from 'qrcode.react';
import { Loader2, CheckCircle2, XCircle, Smartphone } from 'lucide-react';

import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import {
  generateBilibiliQr,
  pollBilibiliQr,
  type CredentialSaveScope,
} from '@/server/functions/credentials';

interface BilibiliQrLoginDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  scope: CredentialSaveScope;
  onSuccess?: () => void;
}

type QrStatus =
  | 'loading'
  | 'ready'
  | 'scanned'
  | 'success'
  | 'expired'
  | 'error';

export function BilibiliQrLoginDialog({
  open,
  onOpenChange,
  scope,
  onSuccess,
}: BilibiliQrLoginDialogProps) {
  const { i18n } = useLingui();
  const [qrUrl, setQrUrl] = useState<string | null>(null);
  const [authCode, setAuthCode] = useState<string | null>(null);
  const [status, setStatus] = useState<QrStatus>('loading');
  const [message, setMessage] = useState<string>('');

  const generateQr = useCallback(async () => {
    setStatus('loading');
    setMessage('');
    try {
      const result = await generateBilibiliQr();
      setQrUrl(result.url);
      setAuthCode(result.auth_code);
      setStatus('ready');
    } catch (e) {
      setStatus('error');
      setMessage(e instanceof Error ? e.message : 'Failed to generate QR code');
    }
  }, []);

  // Generate QR code when dialog opens
  useEffect(() => {
    if (open) {
      generateQr();
    } else {
      // Reset state when closing
      setQrUrl(null);
      setAuthCode(null);
      setStatus('loading');
      setMessage('');
    }
  }, [open, generateQr]);

  // Poll for login status
  useEffect(() => {
    if (!open || !authCode || status === 'success' || status === 'error') {
      return;
    }

    const poll = async () => {
      try {
        const result = await pollBilibiliQr({
          data: { auth_code: authCode, scope },
        });

        if (result.success) {
          setStatus('success');
          setMessage(result.message);
          onSuccess?.();
          // Auto close after success
          setTimeout(() => onOpenChange(false), 1500);
        } else if (result.status === 'expired') {
          setStatus('expired');
          setMessage(result.message);
        } else if (result.status === 'scanned') {
          setStatus('scanned');
          setMessage(result.message);
        }
        // For 'not_scanned', keep polling
      } catch (e) {
        setStatus('error');
        setMessage(e instanceof Error ? e.message : 'Failed to poll status');
      }
    };

    // Poll every 2 seconds
    const interval = setInterval(poll, 2000);
    // Also poll immediately
    poll();

    return () => clearInterval(interval);
  }, [open, authCode, scope, status, onSuccess, onOpenChange]);

  const getStatusContent = () => {
    switch (status) {
      case 'loading':
        return (
          <div className="flex flex-col items-center gap-3 py-8">
            <Loader2 className="h-12 w-12 animate-spin text-muted-foreground" />
            <p className="text-muted-foreground">
              <Trans>Generating QR code...</Trans>
            </p>
          </div>
        );
      case 'ready':
        return (
          <div className="flex flex-col items-center gap-4">
            <div className="rounded-lg bg-white p-4">
              {qrUrl && <QRCodeSVG value={qrUrl} size={200} />}
            </div>
            <div className="flex items-center gap-2 text-sm text-muted-foreground">
              <Smartphone className="h-4 w-4" />
              <Trans>Scan with Bilibili mobile app</Trans>
            </div>
          </div>
        );
      case 'scanned':
        return (
          <div className="flex flex-col items-center gap-4">
            <div className="rounded-lg bg-white p-4 opacity-50">
              {qrUrl && <QRCodeSVG value={qrUrl} size={200} />}
            </div>
            <div className="flex items-center gap-2 text-sm text-primary">
              <Loader2 className="h-4 w-4 animate-spin" />
              <Trans>Scanned! Please confirm on your phone</Trans>
            </div>
          </div>
        );
      case 'success':
        return (
          <div className="flex flex-col items-center gap-3 py-8">
            <CheckCircle2 className="h-12 w-12 text-green-500" />
            <p className="text-green-600 font-medium">
              <Trans>Login successful!</Trans>
            </p>
          </div>
        );
      case 'expired':
        return (
          <div className="flex flex-col items-center gap-4 py-8">
            <XCircle className="h-12 w-12 text-orange-500" />
            <p className="text-muted-foreground">
              <Trans>QR code expired</Trans>
            </p>
            <Button onClick={generateQr} variant="outline">
              <Trans>Generate new QR code</Trans>
            </Button>
          </div>
        );
      case 'error':
        return (
          <div className="flex flex-col items-center gap-4 py-8">
            <XCircle className="h-12 w-12 text-red-500" />
            <p className="text-red-600">
              {message || i18n._(msg`An error occurred`)}
            </p>
            <Button onClick={generateQr} variant="outline">
              <Trans>Try again</Trans>
            </Button>
          </div>
        );
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>
            <Trans>Bilibili QR Login</Trans>
          </DialogTitle>
          <DialogDescription>
            <Trans>
              Scan the QR code with Bilibili app to login and save credentials
            </Trans>
          </DialogDescription>
        </DialogHeader>
        <div className="flex justify-center py-4">{getStatusContent()}</div>
      </DialogContent>
    </Dialog>
  );
}
