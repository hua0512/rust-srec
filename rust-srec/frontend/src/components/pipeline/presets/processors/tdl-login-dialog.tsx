import { useState, useEffect, useRef } from 'react';
import { useLingui } from '@lingui/react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import {
  Loader2,
  Terminal,
  Send,
  StopCircle,
  CheckCircle2,
  XCircle,
} from 'lucide-react';
import { QRCodeSVG } from 'qrcode.react';

import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { ScrollArea } from '@/components/ui/scroll-area';
import {
  startTdlLogin,
  getTdlStatus,
  getTdlLoginStatus,
  sendTdlLoginInput,
  cancelTdlLogin,
} from '@/server/functions/tdl';
import { Badge } from '@/components/ui/badge';
import { cn } from '@/lib/utils';
import { useMutation, useQuery } from '@tanstack/react-query';
import { toast } from 'sonner';
import { type TdlLoginType } from '@/api/schemas/tdl';

interface TdlLoginDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  tdlPath?: string;
  workingDir?: string;
  env?: Record<string, string>;
  namespace?: string;
  storage?: string;
  allowPassword?: boolean;
  loginType?: TdlLoginType;
  telegramDesktopDir?: string;
  loginArgs?: string[];
}

function extractFirstUrl(text: string): string | null {
  const match = text.match(
    /(tg:\/\/[^\s)]+|https?:\/\/[^\s)]+|t\.me\/[^\s)]+)/i,
  );
  const raw = match?.[0];
  if (!raw) return null;
  if (raw.toLowerCase().startsWith('t.me/')) return `https://${raw}`;
  return raw;
}

const stripAnsi = (text: string): string => {
  // eslint-disable-next-line no-control-regex
  const pattern = [
    '[',
    String.fromCharCode(27),
    String.fromCharCode(155),
    '][[()#;?]*(?:[0-9]{1,4}(?:;[0-9]{0,4})*)?[0-9A-ORZcf-nqry=><]',
  ].join('');
  const ansiRegex = new RegExp(pattern, 'g');
  return text.replace(ansiRegex, '');
};

function needsTelegramDesktopDir(outputText: string): boolean {
  const lower = outputText.toLowerCase();
  return (
    lower.includes('telegram desktop') &&
    (lower.includes('no data found') ||
      lower.includes('please specify path') ||
      lower.includes('`-d`') ||
      lower.includes(' -d '))
  );
}

export function TdlLoginDialog({
  open,
  onOpenChange,
  tdlPath,
  workingDir,
  env,
  namespace,
  storage,
  loginType: loginTypeProp,
  telegramDesktopDir,
  loginArgs,
}: TdlLoginDialogProps) {
  const { i18n } = useLingui();
  const [sessionId, setSessionId] = useState<string | null>(null);
  const [inputTask, setInputTask] = useState('');
  const [isSensitive, setIsSensitive] = useState(false);
  const allowPassword = false;
  const [desktopDir, setDesktopDir] = useState(telegramDesktopDir ?? '');
  const [toolStatusText, setToolStatusText] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);

  const loginType: TdlLoginType = loginTypeProp ?? 'auto';

  useEffect(() => {
    if (open && !sessionId) {
      setDesktopDir(telegramDesktopDir ?? '');
    }
  }, [open, sessionId, telegramDesktopDir]);

  // Poll for status
  const { data: statusData, refetch: refetchStatus } = useQuery({
    queryKey: ['tdl', 'login', sessionId],
    queryFn: async () => {
      if (!sessionId) {
        throw new Error('Missing session id');
      }
      return await getTdlLoginStatus({ data: sessionId });
    },
    enabled: !!sessionId && open,
    refetchInterval: (query) => {
      const status = query.state.data?.status;
      if (status === 'running') return 1000;
      return false;
    },
  });

  const startMutation = useMutation({
    mutationFn: startTdlLogin,
    onSuccess: (data) => {
      setSessionId(data.session_id);
      toast.success(i18n._(msg`TDL login session started`));
    },
    onError: (e) => {
      toast.error(i18n._(msg`Failed to start login: ${e.message}`));
    },
  });

  const sendMutation = useMutation({
    mutationFn: sendTdlLoginInput,
    onSuccess: () => {
      setInputTask('');
      setIsSensitive(false);
      refetchStatus();
    },
    onError: (e) => {
      toast.error(i18n._(msg`Failed to send input: ${e.message}`));
    },
  });

  const cancelMutation = useMutation({
    mutationFn: cancelTdlLogin,
    onSuccess: () => {
      setSessionId(null);
      onOpenChange(false);
    },
  });

  const toolStatusMutation = useMutation({
    mutationFn: getTdlStatus,
    onSuccess: (data) => {
      const version = data.version ? ` (${data.version})` : '';
      setToolStatusText(`${data.login_state}${version}`);

      if (!data.binary_ok) {
        toast.error(i18n._(msg`TDL not found`));
        return;
      }
      if (data.login_state === 'logged_in') {
        toast.success(i18n._(msg`TDL is logged in${version}`));
        return;
      }
      if (data.login_state === 'not_logged_in') {
        toast(i18n._(msg`TDL is not logged in${version}`));
        return;
      }
      toast(i18n._(msg`TDL status unknown${version}`));
    },
    onError: (e) => {
      toast.error(i18n._(msg`Failed to check TDL status: ${e.message}`));
    },
  });

  // Reset state on close
  useEffect(() => {
    if (!open) {
      if (sessionId) {
        cancelTdlLogin({ data: sessionId }).catch(() => {});
      }
      setSessionId(null);
      setInputTask('');
      setIsSensitive(false);
      setToolStatusText(null);
    }
  }, [open, sessionId]);

  // Scroll to bottom when output changes
  useEffect(() => {
    if (scrollRef.current) {
      const scrollContainer = scrollRef.current.querySelector(
        '[data-radix-scroll-area-viewport]',
      );
      if (scrollContainer) {
        scrollContainer.scrollTop = scrollContainer.scrollHeight;
      }
    }
  }, [statusData?.output]);

  const handleSend = (e?: React.FormEvent) => {
    e?.preventDefault();
    if (!sessionId || !inputTask.trim() || sendMutation.isPending) return;

    sendMutation.mutate({
      data: {
        sessionId,
        input: {
          text: inputTask,
          sensitive: isSensitive,
        },
      },
    });
  };

  const status = statusData?.status;
  const isRunning = status === 'running';

  // Detect sensitive prompts (password, 2FA)
  useEffect(() => {
    if (statusData?.output) {
      const lastOutput = statusData.output.join('').toLowerCase();
      if (
        lastOutput.includes('password') ||
        lastOutput.includes('2fa') ||
        lastOutput.includes('code')
      ) {
        // Best effort hint for sensitive input
        // Simple logic: if it mentions password, it's probably sensitive
        if (lastOutput.includes('password')) {
          setIsSensitive(true);
        }
      }
    }
  }, [statusData?.output]);

  const outputText = statusData?.output?.join('') ?? '';
  const qrUrl = extractFirstUrl(outputText);
  const showDesktopHint = needsTelegramDesktopDir(outputText);

  const computeGlobalArgs = (): string[] => {
    const args: string[] = [];
    if (namespace && namespace.trim()) {
      args.push('--ns', namespace.trim());
    }
    if (storage && storage.trim()) {
      args.push('--storage', storage.trim());
    }
    return args;
  };

  const startLogin = (mode: 'qr' | 'code' | 'desktop') => {
    const computedLoginArgs: string[] = [];

    if (mode === 'qr') {
      computedLoginArgs.push('-T', 'qr');
    }

    if (mode === 'code') {
      computedLoginArgs.push('-T', 'code');
    }

    if (mode === 'desktop') {
      computedLoginArgs.push('-T', 'desktop');
      if (desktopDir.trim()) {
        computedLoginArgs.push('-d', desktopDir.trim());
      }
    }

    if (Array.isArray(loginArgs) && loginArgs.length > 0) {
      computedLoginArgs.push(...loginArgs);
    }

    startMutation.mutate({
      data: {
        tdl_path: tdlPath,
        working_dir: workingDir,
        env: env ?? {},
        global_args: computeGlobalArgs(),
        allow_password: allowPassword,
        login_args: computedLoginArgs,
      },
    });
  };

  const startConfiguredLogin = () => {
    if (loginType === 'desktop') {
      startLogin('desktop');
      return;
    }
    if (loginType === 'code') {
      startLogin('code');
      return;
    }
    // auto + qr => QR first
    startLogin('qr');
  };

  const startLabel =
    loginType === 'desktop' ? (
      <Trans>Start Desktop Login</Trans>
    ) : loginType === 'code' ? (
      <Trans>Start Phone & Code Login</Trans>
    ) : (
      <Trans>Start QR Login</Trans>
    );

  const statusLower = (() => {
    if (!status) return '';
    if (typeof status === 'string') return status;
    if ('failed' in status) return status.failed.message.toLowerCase();
    return '';
  })();

  const isTdlNotFound =
    statusLower.includes('failed to start tdl') &&
    (statusLower.includes('not found') ||
      statusLower.includes('cannot find the file') ||
      statusLower.includes('no such file') ||
      statusLower.includes('program'));

  const isPasswordBlocked =
    statusLower.includes('requires a password') ||
    statusLower.includes('password/2fa') ||
    statusLower.includes('2fa secret') ||
    ((outputText.toLowerCase().includes('2fa password') ||
      outputText.toLowerCase().includes('cloud password')) &&
      !allowPassword);

  const isWindowsError = outputText.includes('Incorrect function');

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-4xl h-[90vh] flex flex-col p-0 overflow-hidden border-none shadow-2xl">
        <DialogHeader className="p-6 pb-2">
          <div className="flex items-center justify-between">
            <div className="space-y-1">
              <DialogTitle className="flex items-center gap-2 text-xl font-bold">
                <div className="p-1.5 rounded-lg bg-primary/10 text-primary">
                  <Terminal className="h-5 w-5" />
                </div>
                <Trans>TDL Interactive Login</Trans>
              </DialogTitle>
              <DialogDescription>
                <Trans>
                  Complete the Telegram login process in this terminal.
                </Trans>
              </DialogDescription>
            </div>
            <div className="flex items-center gap-3">
              {status && (
                <Badge
                  variant={isRunning ? 'default' : 'secondary'}
                  className={cn(
                    'capitalize px-3 py-1',
                    isRunning &&
                      'animate-pulse bg-green-500 hover:bg-green-600',
                  )}
                >
                  {typeof status === 'string' ? status : Object.keys(status)[0]}
                </Badge>
              )}
              <div className="text-[10px] text-muted-foreground">
                <Trans>Telegram 2FA via web is unsupported</Trans>
              </div>
            </div>
          </div>

          {!sessionId && (
            <div className="pt-3 flex items-center gap-2">
              <Button
                type="button"
                onClick={startConfiguredLogin}
                disabled={startMutation.isPending}
                className="h-9"
              >
                {startMutation.isPending ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  startLabel
                )}
              </Button>
              <Button
                type="button"
                variant="outline"
                onClick={() =>
                  toolStatusMutation.mutate({
                    data: {
                      tdl_path: tdlPath,
                      working_dir: workingDir,
                      env: env ?? {},
                      global_args: computeGlobalArgs(),
                    },
                  })
                }
                disabled={toolStatusMutation.isPending}
                className="h-9"
              >
                {toolStatusMutation.isPending ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  <Trans>Check Login</Trans>
                )}
              </Button>
              {(loginType === 'auto' || loginType === 'qr') && (
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => startLogin('code')}
                  disabled={startMutation.isPending}
                  className="h-9"
                >
                  <Trans>Use Phone & Code</Trans>
                </Button>
              )}
              {(loginType === 'desktop' || loginType === 'code') && (
                <Button
                  type="button"
                  variant="outline"
                  onClick={() => startLogin('qr')}
                  disabled={startMutation.isPending}
                  className="h-9"
                >
                  <Trans>Use QR Login</Trans>
                </Button>
              )}
            </div>
          )}
        </DialogHeader>

        <div className="flex-1 px-6 min-h-0 flex flex-col">
          {(qrUrl ||
            showDesktopHint ||
            isTdlNotFound ||
            isPasswordBlocked ||
            isWindowsError ||
            toolStatusText) && (
            <div className="mb-3 space-y-2">
              {isWindowsError && (
                <div className="rounded-xl border border-red-500/20 bg-red-500/5 p-3">
                  <div className="text-xs font-medium text-red-600">
                    <Trans>Windows Console Error</Trans>
                  </div>
                  <div className="text-[11px] text-muted-foreground mt-1">
                    <Trans>
                      Interactive password entry via this API is not supported
                      on Windows. Please use "Desktop Login" (import tdata) or
                      log in once in a regular terminal.
                    </Trans>
                  </div>
                </div>
              )}
              {toolStatusText && (
                <div className="flex items-center justify-between rounded-xl border border-border/40 bg-muted/20 p-3">
                  <div className="text-xs font-medium">
                    <Trans>TDL Status</Trans>
                  </div>
                  <Badge variant="secondary" className="capitalize">
                    {toolStatusText}
                  </Badge>
                </div>
              )}
              {isTdlNotFound && (
                <div className="rounded-xl border border-red-500/20 bg-red-500/5 p-3">
                  <div className="text-xs font-medium text-red-600">
                    <Trans>TDL binary not found</Trans>
                  </div>
                  <div className="text-[11px] text-muted-foreground mt-1">
                    <Trans>
                      Set "TDL Binary Path" to your `tdl` executable, or make
                      sure `tdl` is in PATH.
                    </Trans>
                  </div>
                </div>
              )}
              {isPasswordBlocked && !allowPassword && (
                <div className="rounded-xl border border-orange-500/20 bg-orange-500/5 p-3">
                  <div className="text-xs font-medium text-orange-600">
                    <Trans>Password / 2FA blocked</Trans>
                  </div>
                  <div className="text-[11px] text-muted-foreground mt-1">
                    <Trans>
                      This login requires a Telegram 2FA password. Enable "Allow
                      2FA" and restart.
                    </Trans>
                  </div>
                </div>
              )}
              {qrUrl && (
                <div className="flex items-center gap-3 rounded-xl border border-border/40 bg-muted/20 p-3">
                  <div className="rounded-lg bg-white p-2">
                    <QRCodeSVG value={qrUrl} size={96} />
                  </div>
                  <div className="min-w-0">
                    <div className="text-xs font-medium">
                      <Trans>QR Code</Trans>
                    </div>
                    <div className="text-[11px] text-muted-foreground break-all">
                      {qrUrl}
                    </div>
                  </div>
                </div>
              )}
              {showDesktopHint && (
                <div className="rounded-xl border border-orange-500/20 bg-orange-500/5 p-3">
                  <div className="text-xs font-medium text-orange-600">
                    <Trans>Telegram Desktop data required</Trans>
                  </div>
                  <div className="mt-2 flex items-center gap-2">
                    <Input
                      value={desktopDir}
                      onChange={(e) => setDesktopDir(e.target.value)}
                      placeholder={i18n._(
                        msg`Path to Telegram Desktop (contains tdata)`,
                      )}
                      className="h-9 font-mono text-xs"
                      disabled={isRunning}
                    />
                    <Button
                      type="button"
                      variant="outline"
                      className="h-9"
                      onClick={() => {
                        if (!desktopDir.trim()) {
                          toast.error(
                            i18n._(
                              msg`Please set Telegram Desktop Directory first`,
                            ),
                          );
                          return;
                        }
                        if (sessionId) {
                          cancelTdlLogin({ data: sessionId }).catch(() => {});
                          setSessionId(null);
                        }
                        startLogin('desktop');
                      }}
                      disabled={startMutation.isPending || isRunning}
                    >
                      <Trans>Restart with -d</Trans>
                    </Button>
                  </div>
                </div>
              )}
            </div>
          )}
          <ScrollArea
            ref={scrollRef}
            className="flex-1 w-full rounded-xl bg-slate-950 p-4 font-mono text-xs text-slate-50 border border-slate-800 shadow-inner"
          >
            {startMutation.isPending && (
              <div className="flex items-center gap-2 text-slate-400 italic">
                <Loader2 className="h-4 w-4 animate-spin" />
                <Trans>Starting tdl process...</Trans>
              </div>
            )}

            {statusData?.output.map((chunk, i) => (
              <pre key={i} className="whitespace-pre-wrap leading-none">
                {stripAnsi(chunk)}
              </pre>
            ))}

            {!isRunning && status && (
              <div
                className={cn(
                  'mt-4 p-3 rounded-lg flex items-center gap-3 border',
                  status === 'cancelled'
                    ? 'bg-orange-500/10 border-orange-500/20 text-orange-400'
                    : typeof status === 'object' && 'exited' in status
                      ? 'bg-blue-500/10 border-blue-500/20 text-blue-400'
                      : 'bg-red-500/10 border-red-500/20 text-red-400',
                )}
              >
                {status === 'cancelled' ? (
                  <StopCircle className="h-5 w-5" />
                ) : typeof status === 'object' && 'exited' in status ? (
                  <CheckCircle2 className="h-5 w-5" />
                ) : (
                  <XCircle className="h-5 w-5" />
                )}
                <div className="font-semibold">
                  {status === 'cancelled' ? (
                    <Trans>Login session cancelled</Trans>
                  ) : typeof status === 'object' && 'exited' in status ? (
                    <Trans>
                      Process exited with code {status.exited.code ?? 0}
                    </Trans>
                  ) : (
                    <Trans>
                      Login failed:{' '}
                      {typeof status === 'object' && 'failed' in status
                        ? status.failed.message
                        : 'Unknown error'}
                    </Trans>
                  )}
                </div>
              </div>
            )}
          </ScrollArea>
        </div>

        <DialogFooter className="p-6 pt-2 bg-muted/30 border-t gap-3 sm:gap-0">
          <form
            onSubmit={handleSend}
            className="flex w-full items-center gap-3"
          >
            <div className="relative flex-1 group">
              <Input
                placeholder={
                  isRunning
                    ? i18n._(msg`Type your answer here...`)
                    : i18n._(msg`Session is not running`)
                }
                value={inputTask}
                onChange={(e) => setInputTask(e.target.value)}
                disabled={!isRunning || sendMutation.isPending}
                type={isSensitive ? 'password' : 'text'}
                className="pr-24 h-12 rounded-xl border-border/50 bg-background/50 focus:bg-background transition-colors"
              />
              <div className="absolute right-2 top-1/2 -translate-y-1/2 flex items-center gap-1">
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() => setIsSensitive(!isSensitive)}
                  disabled={!isRunning}
                  className={cn(
                    'h-8 px-2 text-[10px] font-bold uppercase tracking-wider transition-colors',
                    isSensitive
                      ? 'text-orange-500 hover:text-orange-600 bg-orange-500/10 hover:bg-orange-500/20'
                      : 'text-muted-foreground hover:text-foreground',
                  )}
                >
                  {isSensitive ? i18n._(msg`Sensitive`) : i18n._(msg`Normal`)}
                </Button>
              </div>
            </div>
            <Button
              type="submit"
              disabled={
                !isRunning || !inputTask.trim() || sendMutation.isPending
              }
              className="h-12 px-6 rounded-xl shadow-lg shadow-primary/20 transition-all hover:scale-[1.02] active:scale-[0.98]"
            >
              {sendMutation.isPending ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : (
                <>
                  <Trans>Send</Trans>
                  <Send className="ml-2 h-4 w-4" />
                </>
              )}
            </Button>
            {isRunning && (
              <Button
                type="button"
                variant="outline"
                size="icon"
                onClick={() => cancelMutation.mutate({ data: sessionId! })}
                disabled={cancelMutation.isPending}
                className="h-12 w-12 rounded-xl border-red-200 hover:border-red-300 hover:bg-red-50 text-red-600 dark:border-red-900/30 dark:hover:bg-red-900/20"
              >
                <StopCircle className="h-5 w-5" />
              </Button>
            )}
            {!isRunning && (
              <Button
                type="button"
                variant="outline"
                onClick={() => onOpenChange(false)}
                className="h-12 px-6 rounded-xl"
              >
                <Trans>Close</Trans>
              </Button>
            )}
          </form>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
