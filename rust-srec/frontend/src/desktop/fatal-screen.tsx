import { useState } from 'react';
import {
  Check,
  ChevronDown,
  CircleAlert,
  Copy,
  Cpu,
  Database,
  FileText,
  FolderOpen,
  Globe,
  HardDrive,
  Layers,
  Loader2,
  Lock,
  Power,
  RefreshCw,
  RotateCcw,
  Server,
  ShieldAlert,
  Terminal,
  Wrench,
} from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Badge } from '@/components/ui/badge';

export type BootFailureStage =
  | 'logging'
  | 'database'
  | 'migrations'
  | 'backend'
  | 'services'
  | 'api_server'
  | 'window';

export type BootFailureKind =
  | 'database_busy'
  | 'permission_denied'
  | 'storage_full'
  | 'database_corrupt'
  | 'migration_failed'
  | 'logging_failed'
  | 'service_failed'
  | 'api_server_failed'
  | 'window_failed'
  | 'unknown';

export interface BootFailurePayload {
  stage: BootFailureStage;
  kind: BootFailureKind;
  title: string;
  message: string;
  details: string;
}

interface FatalFailure extends BootFailurePayload {
  origin: 'native' | 'frontend';
}

const bootFailureStages = new Set<BootFailureStage>([
  'logging',
  'database',
  'migrations',
  'backend',
  'services',
  'api_server',
  'window',
]);

const bootFailureKinds = new Set<BootFailureKind>([
  'database_busy',
  'permission_denied',
  'storage_full',
  'database_corrupt',
  'migration_failed',
  'logging_failed',
  'service_failed',
  'api_server_failed',
  'window_failed',
  'unknown',
]);

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function readStage(value: unknown): BootFailureStage {
  return typeof value === 'string' &&
    bootFailureStages.has(value as BootFailureStage)
    ? (value as BootFailureStage)
    : 'backend';
}

function readKind(value: unknown): BootFailureKind {
  return typeof value === 'string' &&
    bootFailureKinds.has(value as BootFailureKind)
    ? (value as BootFailureKind)
    : 'unknown';
}

export function parseBootFailure(value: unknown): FatalFailure | null {
  if (typeof value === 'string' && value.trim()) {
    return {
      origin: 'native',
      stage: 'backend',
      kind: 'unknown',
      title: 'Rust-Srec could not finish starting',
      message:
        'Restart the application. If the problem continues, copy the details and review the logs.',
      details: value,
    };
  }

  if (
    !isRecord(value) ||
    typeof value.title !== 'string' ||
    typeof value.message !== 'string' ||
    typeof value.details !== 'string'
  ) {
    return null;
  }

  return {
    origin: 'native',
    stage: readStage(value.stage),
    kind: readKind(value.kind),
    title: value.title,
    message: value.message,
    details: value.details,
  };
}

export function createFrontendFailure(error: unknown): FatalFailure {
  const details =
    error instanceof Error
      ? `${error.name}: ${error.message}\n\n${error.stack ?? ''}`.trim()
      : String(error);

  return {
    origin: 'frontend',
    stage: 'window',
    kind: 'window_failed',
    title: 'The desktop interface could not load',
    message:
      'The local service may still be running. Reload the interface, or restart the application if the problem continues.',
    details,
  };
}

const stageLabels: Record<BootFailureStage, string> = {
  logging: 'Logging Subsystem',
  database: 'Database Engine',
  migrations: 'Database Migration',
  backend: 'Core Backend',
  services: 'Background Services',
  api_server: 'Local API Server',
  window: 'Desktop Interface',
};

const kindLabels: Record<BootFailureKind, string> = {
  database_busy: 'Database Locked',
  permission_denied: 'Permission Denied',
  storage_full: 'Storage Full',
  database_corrupt: 'Database Corrupted',
  migration_failed: 'Migration Failed',
  logging_failed: 'Logging Initialization Failed',
  service_failed: 'Service Startup Failed',
  api_server_failed: 'API Server Failed',
  window_failed: 'Interface Exception',
  unknown: 'Unexpected Error',
};

function getStageIcon(stage: BootFailureStage, kind: BootFailureKind) {
  if (kind === 'permission_denied') return Lock;
  if (kind === 'storage_full') return HardDrive;

  switch (stage) {
    case 'database':
    case 'migrations':
      return Database;
    case 'logging':
      return FileText;
    case 'backend':
      return Server;
    case 'services':
      return Layers;
    case 'api_server':
      return Globe;
    case 'window':
      return ShieldAlert;
    default:
      return CircleAlert;
  }
}

function getTroubleshootingTip(
  stage: BootFailureStage,
  kind: BootFailureKind,
): string {
  switch (kind) {
    case 'database_busy':
      return 'Another instance of Rust-Srec or an external SQLite browser may be holding a lock on the database. Ensure no duplicate processes are active in the background.';
    case 'permission_denied':
      return 'Rust-Srec lacks sufficient permissions to read or write in its application data directory. Check file and folder ownership or try running with proper user privileges.';
    case 'storage_full':
      return 'The target drive has run out of storage space. Free up disk space on the system drive to allow SQLite write-ahead logging and database operations.';
    case 'database_corrupt':
      return 'The database disk image is malformed or corrupted. Before taking further recovery action, create a backup copy of your existing data directory.';
    case 'migration_failed':
      return 'A database schema migration encountered an unexpected error. Reviewing the detailed error logs in the log directory can help diagnose the schema issue.';
    case 'logging_failed':
      return 'The logging subsystem was unable to create or initialize log files. Ensure the application has write access to the configured log directory.';
    case 'service_failed':
      return 'A critical background subsystem failed to start properly. Check the diagnostic stack trace below for error details.';
    case 'api_server_failed':
      return 'The local API server was unable to bind to its configured port. Check if another service is occupying the port or if firewall restrictions apply.';
    case 'window_failed':
      return 'An unhandled script exception occurred in the desktop webview. Reloading the interface will reset UI state without interrupting active recordings.';
    default:
      if (stage === 'database' || stage === 'migrations') {
        return 'A database initialization issue occurred. Back up your database files before attempting repair.';
      }
      return 'An unexpected startup failure occurred. You can attempt a restart, inspect the log directory, or copy the diagnostics below for support.';
  }
}

function diagnosticText(failure: FatalFailure): string {
  return [
    '========================================',
    ' Rust-Srec Desktop Diagnostic Report',
    '========================================',
    `Timestamp : ${new Date().toISOString()}`,
    `Origin    : ${failure.origin.toUpperCase()}`,
    `Stage     : ${stageLabels[failure.stage]} (${failure.stage})`,
    `Kind      : ${kindLabels[failure.kind]} (${failure.kind})`,
    `Title     : ${failure.title}`,
    `Message   : ${failure.message}`,
    '----------------------------------------',
    'Diagnostic Details & Stack Trace:',
    '----------------------------------------',
    failure.details,
    '========================================',
  ].join('\n');
}

async function invokeRecovery(
  command: string,
  args?: Record<string, unknown>,
): Promise<void> {
  const { invoke } = await import('@tauri-apps/api/core');
  await invoke(command, args);
}

async function copyDiagnostic(failure: FatalFailure): Promise<void> {
  if (!navigator.clipboard?.writeText) {
    throw new Error('Clipboard access is not available');
  }
  await navigator.clipboard.writeText(diagnosticText(failure));
}

type PendingAction = 'restart' | 'data' | 'logs' | 'copy' | 'quit' | null;

export function FatalScreen({ failure }: { failure: FatalFailure }) {
  const [pending, setPending] = useState<PendingAction>(null);
  const [copied, setCopied] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  const isNativeFailure = failure.origin === 'native';
  const showDataFolder =
    isNativeFailure &&
    (failure.stage === 'database' ||
      failure.stage === 'migrations' ||
      failure.kind === 'permission_denied' ||
      failure.kind === 'storage_full');

  const StageIcon = getStageIcon(failure.stage, failure.kind);
  const troubleshootingTip = getTroubleshootingTip(failure.stage, failure.kind);

  const runAction = async (
    action: Exclude<PendingAction, null>,
    callback: () => Promise<void>,
  ) => {
    setPending(action);
    setActionError(null);
    try {
      await callback();
    } catch (error) {
      setActionError(error instanceof Error ? error.message : String(error));
    } finally {
      setPending(null);
    }
  };

  const restart = () =>
    runAction('restart', () => invokeRecovery('restart_desktop'));
  const quit = () => runAction('quit', () => invokeRecovery('quit_desktop'));
  const openLocation = (location: 'data' | 'logs') =>
    runAction(location, () =>
      invokeRecovery('open_desktop_recovery_location', { location }),
    );
  const copy = () =>
    runAction('copy', async () => {
      await copyDiagnostic(failure);
      setCopied(true);
      setTimeout(() => setCopied(false), 3000);
    });

  return (
    <main className="relative flex min-h-screen flex-col items-center justify-center overflow-x-hidden bg-background px-4 py-8 text-foreground selection:bg-destructive/20 selection:text-destructive sm:px-6 lg:px-8">
      {/* Ambient background glow & subtle grid pattern */}
      <div
        className="pointer-events-none absolute inset-0 bg-[radial-gradient(circle_at_50%_0%,rgba(239,68,68,0.14),transparent_65%)]"
        aria-hidden="true"
      />
      <div
        className="pointer-events-none absolute inset-0 bg-[linear-gradient(to_right,rgba(120,120,120,0.05)_1px,transparent_1px),linear-gradient(to_bottom,rgba(120,120,120,0.05)_1px,transparent_1px)] bg-[size:2rem_2rem] opacity-60"
        aria-hidden="true"
      />
      <div
        className="pointer-events-none absolute -bottom-32 -right-32 size-96 rounded-full bg-primary/5 blur-3xl"
        aria-hidden="true"
      />

      <section
        className="relative z-10 mx-auto w-full max-w-3xl space-y-6"
        aria-labelledby="fatal-title"
      >
        {/* Top App Header & Safe Mode Status Pill */}
        <div className="flex flex-wrap items-center justify-between gap-3 px-1">
          <div className="flex items-center gap-2.5">
            <div className="flex size-7 items-center justify-center rounded-lg bg-primary/10 border border-primary/20 text-primary">
              <Cpu className="size-4" aria-hidden="true" />
            </div>
            <div className="flex items-center gap-2">
              <span className="font-semibold tracking-tight text-sm text-foreground">
                Rust-Srec
              </span>
              <span className="text-muted-foreground/60 text-xs">/</span>
              <span className="text-xs text-muted-foreground font-medium">
                Desktop Environment
              </span>
            </div>
          </div>

          <div className="flex items-center gap-2">
            <Badge
              variant="outline"
              className="border-destructive/30 bg-destructive/10 text-destructive gap-1.5 px-2.5 py-0.5 text-xs font-medium"
            >
              <span className="size-1.5 rounded-full bg-destructive animate-pulse" />
              Recovery Mode
            </Badge>
            <Badge
              variant="secondary"
              className="font-mono text-[11px] text-muted-foreground font-normal"
            >
              {isNativeFailure ? 'Native Core' : 'Webview UI'}
            </Badge>
          </div>
        </div>

        {/* Hero Card with Glassmorphic styling */}
        <div className="overflow-hidden rounded-2xl border border-border/80 bg-card/95 shadow-2xl backdrop-blur-xl transition-all">
          {/* Top accent gradient bar */}
          <div
            className="h-1.5 w-full bg-gradient-to-r from-destructive via-rose-500 to-amber-500"
            aria-hidden="true"
          />

          <div className="p-6 sm:p-8 space-y-6">
            {/* Header Content: Icon + Badges + Title & Message */}
            <div className="flex items-start gap-5 sm:gap-6">
              {/* Glowing multi-layer Stage Icon */}
              <div className="relative shrink-0">
                <div
                  className="absolute -inset-1 rounded-2xl bg-destructive/20 blur-md opacity-75"
                  aria-hidden="true"
                />
                <div className="relative flex size-14 items-center justify-center rounded-xl border border-destructive/30 bg-destructive/10 text-destructive shadow-inner">
                  <StageIcon className="size-7" aria-hidden="true" />
                </div>
              </div>

              <div className="min-w-0 flex-1 space-y-2">
                {/* Status Chips */}
                <div className="flex flex-wrap items-center gap-2">
                  <Badge
                    variant="outline"
                    className="border-border/80 bg-muted/50 text-foreground font-medium text-xs"
                  >
                    Stage: {stageLabels[failure.stage]}
                  </Badge>
                  <Badge
                    variant="destructive"
                    className="font-mono text-xs font-normal"
                  >
                    {kindLabels[failure.kind]}
                  </Badge>
                </div>

                <h1
                  id="fatal-title"
                  className="text-foreground text-xl font-bold tracking-tight sm:text-2xl"
                >
                  {failure.title}
                </h1>

                <p className="text-muted-foreground text-sm leading-relaxed sm:text-base">
                  {failure.message}
                </p>
              </div>
            </div>

            {/* Contextual Troubleshooting Guidance Card */}
            <div className="flex items-start gap-3 rounded-xl border border-border/70 bg-muted/40 p-4 text-xs sm:text-sm">
              <Wrench
                className="size-4 shrink-0 text-amber-500 dark:text-amber-400 mt-0.5"
                aria-hidden="true"
              />
              <div className="space-y-1">
                <p className="font-semibold text-foreground">
                  Troubleshooting suggestion
                </p>
                <p className="text-muted-foreground leading-relaxed">
                  {troubleshootingTip}
                </p>
              </div>
            </div>

            {/* Action Toolbar */}
            <div className="flex flex-wrap items-center gap-2.5 pt-2">
              {isNativeFailure ? (
                <Button
                  onClick={() => void restart()}
                  disabled={pending !== null}
                  className="shadow-sm font-medium gap-2 min-w-[150px]"
                >
                  {pending === 'restart' ? (
                    <Loader2
                      className="size-4 animate-spin"
                      aria-hidden="true"
                    />
                  ) : (
                    <RotateCcw className="size-4" aria-hidden="true" />
                  )}
                  {pending === 'restart'
                    ? 'Restarting...'
                    : 'Restart application'}
                </Button>
              ) : (
                <Button
                  onClick={() => window.location.reload()}
                  className="shadow-sm font-medium gap-2 min-w-[150px]"
                >
                  <RefreshCw className="size-4" aria-hidden="true" />
                  Reload interface
                </Button>
              )}

              {showDataFolder && (
                <Button
                  variant="outline"
                  onClick={() => void openLocation('data')}
                  disabled={pending !== null}
                  className="gap-2 border-border/80 bg-background/50 hover:bg-muted"
                >
                  {pending === 'data' ? (
                    <Loader2
                      className="size-4 animate-spin"
                      aria-hidden="true"
                    />
                  ) : (
                    <FolderOpen className="size-4" aria-hidden="true" />
                  )}
                  Open data folder
                </Button>
              )}

              <Button
                variant="outline"
                onClick={() => void openLocation('logs')}
                disabled={pending !== null}
                className="gap-2 border-border/80 bg-background/50 hover:bg-muted"
              >
                {pending === 'logs' ? (
                  <Loader2 className="size-4 animate-spin" aria-hidden="true" />
                ) : (
                  <FolderOpen className="size-4" aria-hidden="true" />
                )}
                Open logs
              </Button>

              <Button
                variant="ghost"
                onClick={() => void copy()}
                disabled={pending !== null}
                className="gap-2 hover:bg-muted"
              >
                {copied ? (
                  <Check
                    className="size-4 text-emerald-500"
                    aria-hidden="true"
                  />
                ) : (
                  <Copy className="size-4" aria-hidden="true" />
                )}
                {copied ? 'Details copied' : 'Copy details'}
              </Button>

              <Button
                variant="ghost"
                onClick={() => void quit()}
                disabled={pending !== null}
                className="gap-2 text-muted-foreground hover:text-destructive hover:bg-destructive/10 ml-auto"
              >
                {pending === 'quit' ? (
                  <Loader2 className="size-4 animate-spin" aria-hidden="true" />
                ) : (
                  <Power className="size-4" aria-hidden="true" />
                )}
                Quit
              </Button>
            </div>

            {/* Action Error Alert */}
            {actionError && (
              <div
                className="flex items-center gap-2.5 rounded-lg border border-destructive/30 bg-destructive/10 p-3.5 text-sm text-destructive"
                role="alert"
              >
                <CircleAlert className="size-4 shrink-0" aria-hidden="true" />
                <span className="font-medium">{actionError}</span>
              </div>
            )}
          </div>
        </div>

        {/* Collapsible Technical Details / Terminal Console */}
        <details className="group rounded-2xl border border-border/80 bg-card/90 shadow-md backdrop-blur-xl overflow-hidden transition-colors">
          <summary className="flex cursor-pointer list-none select-none items-center justify-between px-6 py-4 transition-colors hover:bg-muted/40">
            <div className="flex items-center gap-2.5 text-sm font-medium text-foreground">
              <Terminal
                className="size-4 text-muted-foreground"
                aria-hidden="true"
              />
              <span>Technical details</span>
              <Badge
                variant="secondary"
                className="font-mono text-[11px] font-normal"
              >
                {failure.kind}
              </Badge>
            </div>
            <ChevronDown
              className="size-4 text-muted-foreground transition-transform duration-200 group-open:rotate-180"
              aria-hidden="true"
            />
          </summary>

          <div className="border-t border-border/60 bg-muted/20 px-6 pb-6 pt-4 space-y-3">
            {/* Terminal Window Box */}
            <div className="overflow-hidden rounded-xl border border-zinc-800 bg-zinc-950 text-zinc-200 shadow-inner dark:bg-black/90">
              {/* Terminal Window Top Bar */}
              <div className="flex items-center justify-between border-b border-zinc-800/80 bg-zinc-900/80 px-4 py-2 text-xs">
                <div className="flex items-center gap-2">
                  <div className="flex items-center gap-1.5">
                    <span className="size-2.5 rounded-full bg-red-500/80" />
                    <span className="size-2.5 rounded-full bg-amber-500/80" />
                    <span className="size-2.5 rounded-full bg-emerald-500/80" />
                  </div>
                  <span className="font-mono text-[11px] text-zinc-400 ml-2">
                    startup_diagnostics.log
                  </span>
                </div>

                <button
                  type="button"
                  onClick={() => void copy()}
                  className="flex items-center gap-1 text-[11px] text-zinc-400 hover:text-zinc-100 transition-colors font-mono cursor-pointer"
                >
                  {copied ? (
                    <Check className="size-3 text-emerald-400" />
                  ) : (
                    <Copy className="size-3" />
                  )}
                  {copied ? 'Copied' : 'Copy'}
                </button>
              </div>

              {/* Terminal Output */}
              <pre className="max-h-64 overflow-auto p-4 font-mono text-xs leading-relaxed text-zinc-300 whitespace-pre-wrap selection:bg-rose-500/30">
                <span className="text-zinc-500">
                  {`// Origin: ${failure.origin} | Stage: ${failure.stage} | Kind: ${failure.kind}\n\n`}
                </span>
                {failure.details}
              </pre>
            </div>
          </div>
        </details>
      </section>
    </main>
  );
}
