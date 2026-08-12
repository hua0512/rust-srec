import { createLazyFileRoute } from '@tanstack/react-router';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { motion } from 'motion/react';
import { useMemo, useState } from 'react';
import { toast } from 'sonner';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import {
  Bot,
  Check,
  Copy,
  KeyRound,
  Plus,
  ShieldAlert,
  Trash2,
} from 'lucide-react';

import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table';
import { Skeleton } from '@/components/ui/skeleton';
import {
  type ApiKey,
  type ApiKeyAccessLevel,
  createApiKey,
  listApiKeys,
  revokeApiKey,
} from '@/server/functions/apiKeys';

export const Route = createLazyFileRoute('/_authed/_dashboard/config/api-keys')(
  {
    component: ApiKeysPage,
  },
);

const EXPIRY_OPTIONS = [
  { value: 'never', days: null },
  { value: '7', days: 7 },
  { value: '30', days: 30 },
  { value: '90', days: 90 },
  { value: '365', days: 365 },
] as const;

function keyStatus(key: ApiKey): 'active' | 'revoked' | 'expired' {
  if (key.revoked_at) return 'revoked';
  if (key.expires_at && key.expires_at < Date.now()) return 'expired';
  return 'active';
}

function formatMs(ms: number | null | undefined) {
  if (!ms) return null;
  return new Date(ms).toLocaleString();
}

function ApiKeysPage() {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();

  const { data: keys, isLoading } = useQuery({
    queryKey: ['auth', 'api-keys'],
    queryFn: () => listApiKeys(),
  });

  const [createOpen, setCreateOpen] = useState(false);
  const [revokeTarget, setRevokeTarget] = useState<ApiKey | null>(null);

  const revokeMutation = useMutation({
    mutationFn: (id: string) => revokeApiKey({ data: id }),
    onSuccess: () => {
      toast.success(i18n._(msg`API key revoked`));
      void queryClient.invalidateQueries({ queryKey: ['auth', 'api-keys'] });
    },
    onError: (error: any) => {
      toast.error(error.message || i18n._(msg`Failed to revoke API key`));
    },
  });

  return (
    <div className="flex flex-col gap-6 pb-20">
      <motion.div
        initial={{ opacity: 0, y: -10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.3 }}
        className="flex flex-wrap items-center justify-between gap-3"
      >
        <div className="space-y-1">
          <h2 className="text-lg font-semibold tracking-tight flex items-center gap-2">
            <KeyRound className="h-5 w-5 text-primary" />
            <Trans>API Keys</Trans>
          </h2>
          <p className="text-sm text-muted-foreground max-w-2xl">
            <Trans>
              Long-lived credentials for programmatic access to the REST API and
              the built-in MCP server. Read-only keys can query data;
              full-access keys can also change configuration.
            </Trans>
          </p>
        </div>
        <Button onClick={() => setCreateOpen(true)} className="gap-2">
          <Plus className="h-4 w-4" />
          <Trans>Create API key</Trans>
        </Button>
      </motion.div>

      <Card>
        <CardContent className="pt-6">
          {isLoading ? (
            <div className="space-y-3">
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
              <Skeleton className="h-10 w-full" />
            </div>
          ) : !keys || keys.length === 0 ? (
            <div className="flex flex-col items-center justify-center gap-2 py-10 text-center">
              <KeyRound className="h-8 w-8 text-muted-foreground/50" />
              <p className="text-sm text-muted-foreground">
                <Trans>
                  No API keys yet. Create one to let scripts or AI assistants
                  access this server.
                </Trans>
              </p>
            </div>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>
                    <Trans>Name</Trans>
                  </TableHead>
                  <TableHead>
                    <Trans>Key</Trans>
                  </TableHead>
                  <TableHead>
                    <Trans>Access</Trans>
                  </TableHead>
                  <TableHead>
                    <Trans>Status</Trans>
                  </TableHead>
                  <TableHead>
                    <Trans>Last used</Trans>
                  </TableHead>
                  <TableHead>
                    <Trans>Expires</Trans>
                  </TableHead>
                  <TableHead className="w-[60px]" />
                </TableRow>
              </TableHeader>
              <TableBody>
                {keys.map((key) => {
                  const status = keyStatus(key);
                  return (
                    <TableRow key={key.id}>
                      <TableCell className="font-medium">{key.name}</TableCell>
                      <TableCell>
                        <code className="rounded bg-muted px-1.5 py-0.5 text-xs">
                          {key.key_prefix}…
                        </code>
                      </TableCell>
                      <TableCell>
                        {key.access_level === 'full' ? (
                          <Badge variant="default">
                            <Trans>Full access</Trans>
                          </Badge>
                        ) : (
                          <Badge variant="secondary">
                            <Trans>Read-only</Trans>
                          </Badge>
                        )}
                      </TableCell>
                      <TableCell>
                        {status === 'active' && (
                          <Badge
                            variant="outline"
                            className="border-emerald-500/40 text-emerald-600 dark:text-emerald-400"
                          >
                            <Trans>Active</Trans>
                          </Badge>
                        )}
                        {status === 'revoked' && (
                          <Badge variant="outline" className="opacity-60">
                            <Trans>Revoked</Trans>
                          </Badge>
                        )}
                        {status === 'expired' && (
                          <Badge
                            variant="outline"
                            className="border-amber-500/40 text-amber-600 dark:text-amber-400"
                          >
                            <Trans>Expired</Trans>
                          </Badge>
                        )}
                      </TableCell>
                      <TableCell className="text-sm text-muted-foreground">
                        {formatMs(key.last_used_at) ?? (
                          <span className="opacity-60">
                            <Trans>Never</Trans>
                          </span>
                        )}
                      </TableCell>
                      <TableCell className="text-sm text-muted-foreground">
                        {formatMs(key.expires_at) ?? (
                          <span className="opacity-60">
                            <Trans>Never</Trans>
                          </span>
                        )}
                      </TableCell>
                      <TableCell>
                        {status === 'active' && (
                          <Button
                            variant="ghost"
                            size="icon"
                            className="h-8 w-8 text-destructive hover:text-destructive"
                            onClick={() => setRevokeTarget(key)}
                            aria-label={i18n._(msg`Revoke API key`)}
                          >
                            <Trash2 className="h-4 w-4" />
                          </Button>
                        )}
                      </TableCell>
                    </TableRow>
                  );
                })}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>

      <McpQuickStart />

      <CreateApiKeyDialog
        open={createOpen}
        onOpenChange={setCreateOpen}
        onCreated={() =>
          void queryClient.invalidateQueries({ queryKey: ['auth', 'api-keys'] })
        }
      />

      <AlertDialog
        open={revokeTarget !== null}
        onOpenChange={(open) => !open && setRevokeTarget(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              <Trans>Revoke API key?</Trans>
            </AlertDialogTitle>
            <AlertDialogDescription>
              <Trans>
                "{revokeTarget?.name}" will stop working immediately. This
                cannot be undone.
              </Trans>
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>
              <Trans>Cancel</Trans>
            </AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              onClick={() => {
                if (revokeTarget) revokeMutation.mutate(revokeTarget.id);
                setRevokeTarget(null);
              }}
            >
              <Trans>Revoke</Trans>
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function CreateApiKeyDialog({
  open,
  onOpenChange,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCreated: () => void;
}) {
  const { i18n } = useLingui();
  const [name, setName] = useState('');
  const [accessLevel, setAccessLevel] =
    useState<ApiKeyAccessLevel>('read_only');
  const [expiry, setExpiry] = useState<string>('never');
  const [createdKey, setCreatedKey] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const createMutation = useMutation({
    mutationFn: () => {
      const option = EXPIRY_OPTIONS.find((o) => o.value === expiry);
      const expires_at = option?.days
        ? Date.now() + option.days * 24 * 60 * 60 * 1000
        : null;
      return createApiKey({
        data: { name: name.trim(), access_level: accessLevel, expires_at },
      });
    },
    onSuccess: (response) => {
      setCreatedKey(response.api_key);
      onCreated();
    },
    onError: (error: any) => {
      toast.error(error.message || i18n._(msg`Failed to create API key`));
    },
  });

  const close = () => {
    onOpenChange(false);
    // Delay the reset so the raw key does not flash away mid-animation.
    setTimeout(() => {
      setName('');
      setAccessLevel('read_only');
      setExpiry('never');
      setCreatedKey(null);
      setCopied(false);
    }, 200);
  };

  const copyKey = async () => {
    if (!createdKey) return;
    await navigator.clipboard.writeText(createdKey);
    setCopied(true);
    toast.success(i18n._(msg`API key copied to clipboard`));
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => (next ? onOpenChange(true) : close())}
    >
      <DialogContent className="sm:max-w-md">
        {createdKey ? (
          <>
            <DialogHeader>
              <DialogTitle>
                <Trans>API key created</Trans>
              </DialogTitle>
              <DialogDescription className="flex items-start gap-2 pt-2 text-amber-600 dark:text-amber-400">
                <ShieldAlert className="h-4 w-4 mt-0.5 shrink-0" />
                <Trans>
                  Copy this key now. For security reasons it is shown only once
                  and cannot be recovered later.
                </Trans>
              </DialogDescription>
            </DialogHeader>
            <div className="flex items-center gap-2">
              <code className="flex-1 rounded-lg border bg-muted px-3 py-2 text-xs break-all select-all">
                {createdKey}
              </code>
              <Button
                variant="outline"
                size="icon"
                onClick={copyKey}
                aria-label={i18n._(msg`Copy API key`)}
              >
                {copied ? (
                  <Check className="h-4 w-4 text-emerald-500" />
                ) : (
                  <Copy className="h-4 w-4" />
                )}
              </Button>
            </div>
            <DialogFooter>
              <Button onClick={close}>
                <Trans>Done</Trans>
              </Button>
            </DialogFooter>
          </>
        ) : (
          <>
            <DialogHeader>
              <DialogTitle>
                <Trans>Create API key</Trans>
              </DialogTitle>
              <DialogDescription>
                <Trans>
                  The key inherits your account's permissions, limited by the
                  access level you pick here.
                </Trans>
              </DialogDescription>
            </DialogHeader>
            <div className="space-y-4">
              <div className="space-y-2">
                <Label htmlFor="api-key-name">
                  <Trans>Name</Trans>
                </Label>
                <Input
                  id="api-key-name"
                  value={name}
                  maxLength={100}
                  placeholder={i18n._(msg`e.g. Claude assistant`)}
                  onChange={(e) => setName(e.target.value)}
                />
              </div>
              <div className="space-y-2">
                <Label>
                  <Trans>Access level</Trans>
                </Label>
                <Select
                  value={accessLevel}
                  onValueChange={(v) => setAccessLevel(v as ApiKeyAccessLevel)}
                >
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="read_only">
                      <Trans>Read-only — query data and configuration</Trans>
                    </SelectItem>
                    <SelectItem value="full">
                      <Trans>Full access — also modify configuration</Trans>
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <div className="space-y-2">
                <Label>
                  <Trans>Expires</Trans>
                </Label>
                <Select value={expiry} onValueChange={setExpiry}>
                  <SelectTrigger>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="never">
                      <Trans>Never</Trans>
                    </SelectItem>
                    <SelectItem value="7">
                      <Trans>7 days</Trans>
                    </SelectItem>
                    <SelectItem value="30">
                      <Trans>30 days</Trans>
                    </SelectItem>
                    <SelectItem value="90">
                      <Trans>90 days</Trans>
                    </SelectItem>
                    <SelectItem value="365">
                      <Trans>1 year</Trans>
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </div>
            <DialogFooter>
              <Button variant="outline" onClick={close}>
                <Trans>Cancel</Trans>
              </Button>
              <Button
                onClick={() => createMutation.mutate()}
                disabled={name.trim().length === 0 || createMutation.isPending}
              >
                {createMutation.isPending ? (
                  <Trans>Creating…</Trans>
                ) : (
                  <Trans>Create</Trans>
                )}
              </Button>
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

function McpQuickStart() {
  const { i18n } = useLingui();
  const [copied, setCopied] = useState(false);

  const mcpUrl = useMemo(() => {
    if (typeof window === 'undefined') return '/api/mcp';
    return `${window.location.origin}/api/mcp`;
  }, []);

  const configSnippet = useMemo(
    () =>
      JSON.stringify(
        {
          mcpServers: {
            'rust-srec': {
              url: mcpUrl,
              headers: {
                Authorization: 'Bearer srec_YOUR_API_KEY',
              },
            },
          },
        },
        null,
        2,
      ),
    [mcpUrl],
  );

  const copySnippet = async () => {
    await navigator.clipboard.writeText(configSnippet);
    setCopied(true);
    toast.success(i18n._(msg`Configuration copied to clipboard`));
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2 text-base">
          <Bot className="h-5 w-5 text-primary" />
          <Trans>Connect an AI assistant (MCP)</Trans>
        </CardTitle>
        <CardDescription>
          <Trans>
            This server exposes a built-in MCP endpoint (streamable HTTP). Add
            it to Claude, Cursor, or any MCP client with an API key to let the
            assistant inspect recordings, analyze danmu, and manage
            configuration.
          </Trans>
        </CardDescription>
      </CardHeader>
      <CardContent className="space-y-3">
        <div className="flex items-center gap-2 text-sm">
          <span className="text-muted-foreground shrink-0">
            <Trans>Endpoint</Trans>
          </span>
          <code className="rounded bg-muted px-2 py-1 text-xs break-all">
            {mcpUrl}
          </code>
        </div>
        <div className="relative">
          <pre className="rounded-lg border bg-muted/50 p-4 text-xs overflow-x-auto">
            {configSnippet}
          </pre>
          <Button
            variant="outline"
            size="icon"
            className="absolute right-2 top-2 h-8 w-8"
            onClick={copySnippet}
            aria-label={i18n._(msg`Copy MCP configuration`)}
          >
            {copied ? (
              <Check className="h-4 w-4 text-emerald-500" />
            ) : (
              <Copy className="h-4 w-4" />
            )}
          </Button>
        </div>
        <p className="text-xs text-muted-foreground">
          <Trans>
            Read-only keys can call query tools only; configuration-changing
            tools require a full-access key. The key can also be sent via the
            X-Api-Key header.
          </Trans>
        </p>
      </CardContent>
    </Card>
  );
}
