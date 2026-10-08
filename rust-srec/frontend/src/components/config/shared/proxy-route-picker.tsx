import { useId, useState, type ReactNode } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import {
  useFormContext,
  useWatch,
  type FieldValues,
  type Path,
} from 'react-hook-form';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import type { I18n } from '@lingui/core';
import {
  AlertTriangle,
  Info,
  Monitor,
  Plus,
  Settings2,
  Shield,
} from 'lucide-react';
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Card, CardContent, CardHeader } from '@/components/ui/card';
import { FormField, FormItem, FormMessage } from '@/components/ui/form';
import { Label } from '@/components/ui/label';
import { Callout } from '@/components/shared/callout';
import { isDesktopBuild } from '@/utils/desktop';
import {
  effectiveRouteQueryOptions,
  systemProxyQueryOptions,
} from '@/api/proxies';
import type { EngineConfig } from '@/api/schemas';
import {
  INHERIT_ROUTE,
  type EffectiveRoute,
  type EffectiveRouteQuery,
  type ProxyRoute,
  type SavedProxy,
} from '@/api/schemas/proxies';
import { ProxyEditorDialog } from '@/components/proxies/proxy-editor-dialog';
import {
  DIRECT_LABEL,
  ROUTE_SOURCE_LABELS,
  SYSTEM_LABEL,
  ffmpegCanUse,
  proxyAddress,
  useSavedProxies,
} from '@/components/proxies/proxy-route-label';
import { CONFIG_SELECT_CONTENT, CONFIG_SELECT_TRIGGER } from './config-field';
import { cn } from '@/lib/utils';

/**
 * What choosing Inherit means for the scope being edited, which decides how
 * the option is labelled.
 */
export type ProxyRouteInherit =
  /** The route the next scope out resolves to, shown with its source. */
  | { kind: 'scope'; query: EffectiveRouteQuery }
  /** A template: each streamer follows its own platform. */
  | { kind: 'per-platform' }
  /** The next scope out is not known yet, such as for a streamer being added. */
  | { kind: 'unknown' }
  /** An account: it follows the recording using it. */
  | { kind: 'recording' };

const ADD = '__add__';

function routeValue(route: ProxyRoute | undefined): string {
  if (!route) return '';
  return route.kind === 'proxy' ? `proxy:${route.id}` : route.kind;
}

function parseRouteValue(value: string): ProxyRoute | undefined {
  if (value === 'inherit' || value === 'direct' || value === 'system')
    return { kind: value };
  if (value.startsWith('proxy:')) return { kind: 'proxy', id: value.slice(6) };
  return undefined;
}

function effectiveText(i18n: I18n, route: EffectiveRoute): string {
  const through =
    route.kind === 'proxy'
      ? (route.proxy?.name ?? i18n._(msg`Saved proxy`))
      : i18n._(route.kind === 'direct' ? DIRECT_LABEL : SYSTEM_LABEL);
  const source = i18n._(ROUTE_SOURCE_LABELS[route.source]);
  return `${through} (${source})`;
}

/** The label of the Inherit option for `inherit`. */
function useInheritLabel(inherit: ProxyRouteInherit | undefined): string {
  const { i18n } = useLingui();
  const query = inherit?.kind === 'scope' ? inherit.query : undefined;
  const effective = useQuery({
    ...effectiveRouteQueryOptions(query ?? { scope_type: 'global' }),
    enabled: query !== undefined,
  });
  switch (inherit?.kind) {
    case 'recording':
      return i18n._(msg`Follow the recording's proxy`);
    case 'per-platform':
      return i18n._(msg`Inherit — each streamer's platform setting`);
    case 'scope':
      if (effective.data) {
        const resolved = effectiveText(i18n, effective.data);
        return i18n._(msg`Inherit — ${resolved}`);
      }
      return i18n._(msg`Inherit`);
    default:
      return i18n._(msg`Inherit`);
  }
}

/** Whether `engine` (an engine's id or name) is an FFmpeg engine. */
export function isFfmpegEngine(
  engine: string | null | undefined,
  engines: EngineConfig[] | undefined,
): boolean {
  if (!engine) return false;
  const configured = engines?.find(
    (candidate) => candidate.id === engine || candidate.name === engine,
  );
  if (configured) return configured.engine_type === 'FFMPEG';
  return engine.toLowerCase() === 'ffmpeg';
}

/**
 * Chooses how a scope connects: inherit (unless `inherit` is omitted, as for
 * the global route), directly, through the system proxy, or through a saved
 * proxy, with a shortcut to add one. Notes below the select explain what the
 * choice does when that is not obvious.
 */
export function ProxyRoutePicker({
  id,
  value,
  onChange,
  inherit,
  ffmpeg = false,
  className,
}: {
  id?: string;
  value: ProxyRoute | undefined;
  onChange: (route: ProxyRoute) => void;
  inherit?: ProxyRouteInherit;
  /** The scope records with FFmpeg, which only takes http proxies. */
  ffmpeg?: boolean;
  className?: string;
}) {
  const { i18n } = useLingui();
  const proxies = useSavedProxies();
  const system = useQuery(systemProxyQueryOptions).data;
  const inheritLabel = useInheritLabel(inherit);
  const [adding, setAdding] = useState(false);
  const route = value ?? (inherit ? INHERIT_ROUTE : undefined);
  const selected =
    route?.kind === 'proxy'
      ? proxies.data?.find((proxy) => proxy.id === route.id)
      : undefined;
  const missing =
    route?.kind === 'proxy' && proxies.isSuccess && selected === undefined;
  const systemDetail = system?.detected ? system.url : undefined;

  const triggerText = (() => {
    switch (route?.kind) {
      case undefined:
        return undefined;
      case 'inherit':
        return inheritLabel;
      case 'direct':
        return i18n._(DIRECT_LABEL);
      case 'system':
        return i18n._(SYSTEM_LABEL);
      default:
        return selected?.name ?? i18n._(msg`Saved proxy`);
    }
  })();
  const choose = (proxy: SavedProxy) => {
    setAdding(false);
    onChange({ kind: 'proxy', id: proxy.id });
  };

  return (
    <div className={cn('space-y-2', className)}>
      <Select
        value={routeValue(route)}
        onValueChange={(next) => {
          if (next === ADD) {
            setAdding(true);
            return;
          }
          const parsed = parseRouteValue(next);
          if (parsed) onChange(parsed);
        }}
      >
        <SelectTrigger id={id} className={CONFIG_SELECT_TRIGGER}>
          <SelectValue placeholder={i18n._(msg`Choose how to connect`)}>
            {triggerText && (
              <span className="flex min-w-0 items-center gap-2">
                <span className="truncate">{triggerText}</span>
                {selected && (
                  <span className="truncate font-mono text-xs text-muted-foreground">
                    {proxyAddress(selected)}
                  </span>
                )}
              </span>
            )}
          </SelectValue>
        </SelectTrigger>
        <SelectContent className={CONFIG_SELECT_CONTENT}>
          {inherit && <SelectItem value="inherit">{inheritLabel}</SelectItem>}
          <SelectItem value="direct">
            <OptionText
              label={i18n._(DIRECT_LABEL)}
              detail={i18n._(msg`No proxy`)}
            />
          </SelectItem>
          <SelectItem value="system">
            <OptionText
              label={i18n._(SYSTEM_LABEL)}
              detail={systemDetail ?? i18n._(msg`none detected`)}
              mono={Boolean(systemDetail)}
            />
          </SelectItem>
          {Boolean(proxies.data?.length) && (
            <>
              <SelectSeparator />
              <SelectGroup>
                <SelectLabel>
                  <Trans>Saved proxies</Trans>
                </SelectLabel>
                {proxies.data?.map((proxy) => (
                  <SelectItem key={proxy.id} value={`proxy:${proxy.id}`}>
                    <OptionText
                      label={proxy.name}
                      detail={proxyAddress(proxy)}
                      mono
                    />
                  </SelectItem>
                ))}
              </SelectGroup>
            </>
          )}
          {missing && (
            <SelectItem value={routeValue(route)} disabled>
              {i18n._(msg`Saved proxy`)}
            </SelectItem>
          )}
          <SelectSeparator />
          <SelectItem value={ADD}>
            <span className="flex items-center gap-2 text-primary">
              <Plus className="size-4" />
              <Trans>Add proxy…</Trans>
            </span>
          </SelectItem>
        </SelectContent>
      </Select>
      <div className="flex justify-end px-1">
        <Link
          to="/config/proxies"
          className="inline-flex items-center gap-1 text-[11px] font-medium text-muted-foreground hover:text-foreground hover:underline"
        >
          <Settings2 className="size-3" />
          <Trans>Manage proxies</Trans>
        </Link>
      </div>
      <RouteNotes
        route={route}
        selected={selected}
        missing={missing}
        systemDetected={system?.detected}
        ffmpeg={ffmpeg}
      />
      {adding && (
        <ProxyEditorDialog
          onClose={() => setAdding(false)}
          onSaved={choose}
          onUseExisting={choose}
        />
      )}
    </div>
  );
}

function OptionText({
  label,
  detail,
  mono = false,
}: {
  label: string;
  detail: string;
  mono?: boolean;
}) {
  return (
    <span className="flex min-w-0 items-baseline gap-2">
      <span className="truncate">{label}</span>
      <span
        className={cn(
          'truncate text-xs text-muted-foreground',
          mono && 'font-mono',
        )}
      >
        {detail}
      </span>
    </span>
  );
}

/** What the chosen route does that its name does not say. */
function RouteNotes({
  route,
  selected,
  missing,
  systemDetected,
  ffmpeg,
}: {
  route: ProxyRoute | undefined;
  selected: SavedProxy | undefined;
  missing: boolean;
  systemDetected: boolean | undefined;
  ffmpeg: boolean;
}) {
  const notes: ReactNode[] = [];
  if (route?.kind === 'system') {
    if (systemDetected === false)
      notes.push(
        <Callout key="none" tone="warning" icon={AlertTriangle}>
          <Trans>
            No system proxy was detected when the server started, so this
            connects directly, danmu included.
          </Trans>
        </Callout>,
      );
    if (isDesktopBuild())
      notes.push(
        <Callout key="desktop" tone="info" icon={Monitor}>
          <Trans>
            In the desktop app, the operating system&apos;s proxy setting
            reaches only some requests.
          </Trans>
        </Callout>,
      );
  }
  if (missing)
    notes.push(
      <Callout key="missing" tone="warning" icon={AlertTriangle}>
        <Trans>
          This proxy no longer exists. Requests fail until another is chosen.
        </Trans>
      </Callout>,
    );
  if (route?.kind === 'proxy' && selected && ffmpeg && !ffmpegCanUse(selected))
    notes.push(
      <Callout key="ffmpeg" tone="warning" icon={AlertTriangle}>
        <Trans>
          FFmpeg only supports http proxies, so FFmpeg recordings fail through
          this one. Choose an http proxy or another engine.
        </Trans>
      </Callout>,
    );
  if (!notes.length) return null;
  return <div className="space-y-2">{notes}</div>;
}

/** A `ProxyRoutePicker` bound to the form field `name`. */
export function ProxyRouteField<T extends FieldValues>({
  name,
  inherit,
  ffmpeg,
  label,
}: {
  name: Path<T>;
  inherit?: ProxyRouteInherit;
  ffmpeg?: boolean;
  label?: ReactNode;
}) {
  const { control } = useFormContext<T>();
  const id = useId();
  return (
    <FormField
      control={control}
      name={name}
      render={({ field }) => (
        <FormItem className="space-y-2">
          {label && (
            <Label htmlFor={id} className="px-1 text-sm font-semibold">
              {label}
            </Label>
          )}
          <ProxyRoutePicker
            id={id}
            value={field.value as ProxyRoute | undefined}
            onChange={field.onChange}
            inherit={inherit}
            ffmpeg={ffmpeg}
          />
          <FormMessage />
        </FormItem>
      )}
    />
  );
}

/**
 * The Proxy tab of a configuration editor: how the scope's requests connect.
 * `enginePath` names the form field holding the scope's download engine, so a
 * proxy FFmpeg cannot use can be flagged.
 */
export function ProxyRouteCard<T extends FieldValues>({
  name,
  inherit,
  engines,
  enginePath,
}: {
  name: Path<T>;
  inherit?: ProxyRouteInherit;
  engines?: EngineConfig[];
  enginePath?: Path<T>;
}) {
  const { control } = useFormContext<T>();
  const engine = useWatch({
    control,
    name: (enginePath ?? name) as Path<T>,
    disabled: !enginePath,
  }) as unknown;
  const ffmpeg =
    enginePath !== undefined &&
    isFfmpegEngine(typeof engine === 'string' ? engine : undefined, engines);
  return (
    <Card className="border-border/50 shadow-sm">
      <CardHeader className="px-6 pt-6 pb-3">
        <div className="flex items-center gap-3">
          <div className="rounded-lg bg-indigo-500/10 p-2 text-indigo-600 dark:text-indigo-400">
            <Shield className="h-5 w-5" />
          </div>
          <div>
            <h3 className="text-lg font-semibold leading-none">
              <Trans>Proxy</Trans>
            </h3>
            <p className="mt-1.5 text-sm text-muted-foreground">
              <Trans>
                How checks, recordings, danmu and playback here connect. An
                account with its own proxy setting keeps it.
              </Trans>
            </p>
          </div>
        </div>
      </CardHeader>
      <CardContent className="px-6 pt-0 pb-6">
        <ProxyRouteField<T> name={name} inherit={inherit} ffmpeg={ffmpeg} />
        <p className="mt-3 flex items-start gap-1.5 px-1 text-[11px] text-muted-foreground/80">
          <Info className="mt-px size-3 shrink-0" />
          <Trans>
            Changes apply to the next check or recording; ones already running
            keep their connection.
          </Trans>
        </p>
      </CardContent>
    </Card>
  );
}
