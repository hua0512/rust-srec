import { useEffect, useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { toast } from 'sonner';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import { Textarea } from '@/components/ui/textarea';
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Loader2, TriangleAlert } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { baiduPcsLogin } from '@/server/functions/baidupcs';

interface BaiduPcsLoginDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  binaryPath?: string;
  configDir?: string;
}

/**
 * One-shot Baidu Netdisk login: the pasted cookies / BDUSS+STOKEN are sent
 * once to `POST /api/tools/baidupcs/login` and cleared when the dialog
 * closes — they are never stored client-side. The resulting session lives
 * in BaiduPCS-Go's own config directory on the server.
 */
export function BaiduPcsLoginDialog({
  open,
  onOpenChange,
  binaryPath,
  configDir,
}: BaiduPcsLoginDialogProps) {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();
  const [mode, setMode] = useState<'cookies' | 'bduss'>('cookies');
  const [cookies, setCookies] = useState('');
  const [bduss, setBduss] = useState('');
  const [stoken, setStoken] = useState('');
  const [remember, setRemember] = useState(false);

  useEffect(() => {
    if (!open) {
      setCookies('');
      setBduss('');
      setStoken('');
      setRemember(false);
    }
  }, [open]);

  const loginMutation = useMutation({
    mutationFn: () =>
      baiduPcsLogin({
        data: {
          cookies: mode === 'cookies' ? cookies.trim() || undefined : undefined,
          bduss: mode === 'bduss' ? bduss.trim() || undefined : undefined,
          stoken: mode === 'bduss' ? stoken.trim() || undefined : undefined,
          binary_path: binaryPath || undefined,
          config_dir: configDir || undefined,
          remember,
        },
      }),
    onSuccess: (result) => {
      if (result.success) {
        toast.success(
          result.username
            ? i18n._(msg`Logged in to Baidu Netdisk as ${result.username}`)
            : i18n._(msg`Logged in to Baidu Netdisk`),
        );
        if (result.credentials_stored) {
          toast.info(
            i18n._(msg`Credentials remembered for automatic re-login`),
          );
        } else if (remember) {
          // The login itself worked but the server could not store the
          // material; surface it so "remember" never fails silently.
          toast.warning(
            i18n._(msg`Credentials could not be stored for automatic re-login`),
          );
        }
        void queryClient.invalidateQueries({ queryKey: ['baidupcs-status'] });
        onOpenChange(false);
      } else {
        toast.error(result.message || i18n._(msg`Baidu Netdisk login failed`));
      }
    },
    onError: (error) => {
      toast.error(
        error instanceof Error
          ? error.message
          : i18n._(msg`Baidu Netdisk login failed`),
      );
    },
  });

  const canSubmit =
    mode === 'cookies' ? cookies.trim().length > 0 : bduss.trim().length > 0;
  // The netdisk STOKEN contains uppercase letters; an all-lowercase value
  // is usually the unrelated `bdstoken` copied by mistake.
  const stokenLooksWrong =
    mode === 'bduss' && stoken.trim().length > 0 && !/[A-Z]/.test(stoken);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>
            <Trans>Baidu Netdisk Login</Trans>
          </DialogTitle>
          <DialogDescription>
            <Trans>
              Credentials are handed to BaiduPCS-Go on the server; the login
              session persists in its config directory. The app itself keeps
              them only if you enable remembering below.
            </Trans>
          </DialogDescription>
        </DialogHeader>

        <Tabs
          value={mode}
          onValueChange={(value) => setMode(value as 'cookies' | 'bduss')}
        >
          <TabsList className="grid w-full grid-cols-2">
            <TabsTrigger value="cookies">
              <Trans>Cookies (recommended)</Trans>
            </TabsTrigger>
            <TabsTrigger value="bduss">
              <Trans>BDUSS + STOKEN</Trans>
            </TabsTrigger>
          </TabsList>
        </Tabs>

        {mode === 'cookies' ? (
          <div className="space-y-2">
            <Label htmlFor="baidupcs-cookies">
              <Trans>Cookie string</Trans>
            </Label>
            <Textarea
              id="baidupcs-cookies"
              value={cookies}
              onChange={(e) => setCookies(e.target.value)}
              rows={4}
              autoComplete="off"
              spellCheck={false}
              placeholder={i18n._(msg`BAIDUID=...; BDUSS=...; STOKEN=...; ...`)}
              className="font-mono text-xs"
            />
            <p className="text-xs text-muted-foreground">
              <Trans>
                Copy the full Cookie header from a logged-in Baidu Netdisk page
                (browser DevTools, Network tab). It must include BDUSS and
                STOKEN entries.
              </Trans>
            </p>
          </div>
        ) : (
          <div className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor="baidupcs-bduss">BDUSS</Label>
              <Input
                id="baidupcs-bduss"
                type="password"
                value={bduss}
                onChange={(e) => setBduss(e.target.value)}
                autoComplete="off"
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="baidupcs-stoken">
                <Trans>STOKEN (optional)</Trans>
              </Label>
              <Input
                id="baidupcs-stoken"
                type="password"
                value={stoken}
                onChange={(e) => setStoken(e.target.value)}
                autoComplete="off"
              />
              {stokenLooksWrong && (
                <p className="flex items-start gap-1.5 text-xs text-amber-500">
                  <TriangleAlert className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                  <Trans>
                    This STOKEN contains no uppercase letters — it is likely the
                    wrong value. Copy STOKEN from the netdisk page cookies, not
                    bdstoken.
                  </Trans>
                </p>
              )}
              <p className="text-xs text-muted-foreground">
                <Trans>
                  STOKEN comes from the same netdisk page cookies as BDUSS. Some
                  features fail without it.
                </Trans>
              </p>
            </div>
          </div>
        )}

        <div className="flex items-start gap-2 rounded-lg border border-border/40 bg-muted/20 p-3">
          <Checkbox
            id="baidupcs-remember"
            checked={remember}
            onCheckedChange={(checked) => setRemember(checked === true)}
            className="mt-0.5"
          />
          <div className="space-y-1">
            <Label htmlFor="baidupcs-remember" className="cursor-pointer">
              <Trans>Remember for automatic re-login</Trans>
            </Label>
            <p className="text-xs text-muted-foreground">
              <Trans>
                Stores these credentials on the server (plaintext, like platform
                cookies) so upload jobs can log in again by themselves when the
                session expires. Logging out forgets them.
              </Trans>
            </p>
          </div>
        </div>

        <DialogFooter>
          <Button
            variant="outline"
            onClick={() => onOpenChange(false)}
            disabled={loginMutation.isPending}
          >
            <Trans>Cancel</Trans>
          </Button>
          <Button
            onClick={() => loginMutation.mutate()}
            disabled={!canSubmit || loginMutation.isPending}
          >
            {loginMutation.isPending && (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            )}
            <Trans>Log in</Trans>
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
