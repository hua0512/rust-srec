import type { ReactNode } from 'react';
import { useWatch } from 'react-hook-form';
import type { FieldValues, Path, UseFormReturn } from 'react-hook-form';
import { useMutation, useQuery } from '@tanstack/react-query';
import { toast } from 'sonner';
import {
  FormControl,
  FormDescription,
  FormField,
  FormItem,
} from '@/components/ui/form';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { BellRing, Lock, Radar, RefreshCw, Video } from 'lucide-react';
import { FlagFormField } from '@/components/ui/flag-form-field';
import {
  ConfigFieldLabel,
  ConfigSectionHeading,
  CONFIG_DESCRIPTION,
  CONFIG_SELECT_CONTENT,
  CONFIG_SELECT_TRIGGER,
} from '@/components/config/shared/config-field';
import { configPath } from '@/components/config/shared/form-path';
import { Callout } from '@/components/shared/callout';
import {
  BigoPollingWarning,
  bigoReasonLabel,
} from '@/components/shared/bigo-notifications';
import { bigoNotificationStatusQueryOptions } from '@/api/platform-configs';
import { platformAccountsQueryOptions } from '@/api/credential-profiles';
import type { BigoNotificationStatus } from '@/api/schemas/platform-configs';
import { useHydrated } from '@/hooks/use-hydrated';
import { formatRelativeTime } from '@/lib/datetime';
import { reconnectBigoNotifications } from '@/server/functions';

interface BigoConfigFieldsProps<TFieldValues extends FieldValues> {
  form: UseFormReturn<TFieldValues>;
  /** Path to the object holding this platform's options. */
  fieldName: Path<TFieldValues>;
  /** A template or streamer layer, which can only turn notifications off. */
  inherited?: boolean;
  /** The platform row's ID, on the platform's own editor. */
  platformId?: string;
}

/** The `notification_account` select's value for "no account". */
const NO_ACCOUNT = 'none';

export function BigoConfigFields<TFieldValues extends FieldValues>({
  form,
  fieldName,
  inherited = false,
  platformId,
}: BigoConfigFieldsProps<TFieldValues>) {
  const { i18n } = useLingui();
  return (
    <div className="space-y-12">
      <section className="space-y-6">
        <ConfigSectionHeading icon={Video} accent="sky">
          <Trans>Recording</Trans>
        </ConfigSectionHeading>
        <FlagFormField
          fieldName={configPath<TFieldValues>(fieldName, 'native_stream')}
          inherited={inherited}
          title={<Trans>Record the original stream</Trans>}
          description={
            inherited ? (
              <Trans>
                Off records only the website's stream for these streamers. On
                records the original stream even while it is off in the Bigo
                platform settings.
              </Trans>
            ) : (
              <Trans>
                Records the broadcaster's own stream from BIGO's app servers, in
                its original quality (HEVC or H.264), without an account. The
                website's stream is recorded instead when this fails, for rooms
                with several hosts, and when a stream password is set (the
                platform's default applies to every streamer).
              </Trans>
            )
          }
          // The extractor records the original stream unless this is `false`.
          checked={(value) => value !== false}
        />
      </section>

      <section className="space-y-6">
        <ConfigSectionHeading icon={Lock} accent="sky">
          <Trans>Protection Settings</Trans>
        </ConfigSectionHeading>

        <div className="grid gap-6">
          <FormField
            control={form.control}
            name={configPath<TFieldValues>(fieldName, 'stream_password')}
            render={({ field }) => (
              <FormItem className="space-y-4">
                <ConfigFieldLabel accent="sky">
                  <Trans>Stream Password</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <Input
                    type="password"
                    autoComplete="new-password"
                    {...field}
                    value={field.value || ''}
                    className="bg-background/50 h-10 rounded-xl border-border/50 focus:bg-background transition-all font-mono text-xs shadow-sm"
                    placeholder={i18n._(msg`Password...`)}
                  />
                </FormControl>
                <FormDescription className={CONFIG_DESCRIPTION}>
                  <Trans>
                    Default password for password-protected rooms. Can be
                    overridden per streamer with ?pwd= in the URL.
                  </Trans>
                </FormDescription>
              </FormItem>
            )}
          />
        </div>
      </section>

      <section className="space-y-6">
        <ConfigSectionHeading icon={Radar} accent="sky">
          <Trans>Status Checks</Trans>
        </ConfigSectionHeading>
        <FlagFormField
          fieldName={configPath<TFieldValues>(fieldName, 'batch_detection')}
          inherited={inherited}
          title={<Trans>Batch Detection</Trans>}
          description={
            inherited ? (
              <Trans>
                Off polls these streamers even while BIGO live notifications are
                on in the Bigo platform settings.
              </Trans>
            ) : (
              <Trans>
                Use BIGO live notifications instead of checking each streamer:
                stream starts are noticed within seconds. Needs a signed-in
                account (below).
              </Trans>
            )
          }
          checked={(value) => !!value}
        />
        {!inherited && (
          <NotificationSettings
            form={form}
            fieldName={fieldName}
            platformId={platformId}
          />
        )}
      </section>
    </div>
  );
}

/**
 * The platform-level part of live notifications: the account, what they need before one is
 * chosen, the polling warning, and the saved listener's status.
 */
function NotificationSettings<TFieldValues extends FieldValues>({
  form,
  fieldName,
  platformId,
}: {
  form: UseFormReturn<TFieldValues>;
  fieldName: Path<TFieldValues>;
  platformId: string | undefined;
}) {
  const enabledPath = configPath<TFieldValues>(fieldName, 'batch_detection');
  const accountPath = configPath<TFieldValues>(
    fieldName,
    'notification_account',
  );
  const enabled = !!useWatch({ control: form.control, name: enabledPath });
  const account = useWatch({ control: form.control, name: accountPath });
  const { data: status } = useQuery(bigoNotificationStatusQueryOptions);

  return (
    <div className="space-y-4">
      {enabled && (
        <AccountPicker form={form} name={accountPath} platformId={platformId} />
      )}
      {enabled && !account && (
        <Callout tone="info" icon={BellRing}>
          <Trans>
            Live notifications need a signed-in BIGO account. rust-srec signs
            this account in to BIGO's app service and makes it{' '}
            <strong>
              follow every Bigo streamer you monitor (they can see it)
            </strong>
            , with live alerts on. Turn on live notifications in that account's
            BIGO settings. Use a dedicated account: opening BIGO with the same
            account disconnects rust-srec, and the other way round.
          </Trans>
        </Callout>
      )}
      {(!enabled || !account) && <BigoPollingWarning />}
      {status && status.state !== 'disabled' && (
        <ListenerStatus status={status} />
      )}
    </div>
  );
}

function AccountPicker<TFieldValues extends FieldValues>({
  form,
  name,
  platformId,
}: {
  form: UseFormReturn<TFieldValues>;
  name: Path<TFieldValues>;
  platformId: string | undefined;
}) {
  const { i18n } = useLingui();
  const { data: accounts } = useQuery({
    ...platformAccountsQueryOptions(platformId ?? ''),
    enabled: !!platformId,
  });
  return (
    <FormField
      control={form.control}
      name={name}
      render={({ field }) => {
        const value = typeof field.value === 'string' ? field.value : '';
        const known = accounts?.some((entry) => entry.profile.id === value);
        return (
          <FormItem className="space-y-4">
            <ConfigFieldLabel accent="sky">
              <Trans>Notification Account</Trans>
            </ConfigFieldLabel>
            <Select
              value={value || NO_ACCOUNT}
              onValueChange={(next) =>
                field.onChange(next === NO_ACCOUNT ? null : next)
              }
            >
              <FormControl>
                <SelectTrigger className={CONFIG_SELECT_TRIGGER}>
                  <SelectValue />
                </SelectTrigger>
              </FormControl>
              <SelectContent className={CONFIG_SELECT_CONTENT}>
                <SelectItem value={NO_ACCOUNT}>
                  <Trans context="notification account">None</Trans>
                </SelectItem>
                {accounts?.map(({ profile }) => {
                  const label = profile.label;
                  return (
                    <SelectItem key={profile.id} value={profile.id}>
                      {profile.enabled
                        ? label
                        : i18n._(msg`${label} (disabled)`)}
                    </SelectItem>
                  );
                })}
                {value && accounts && !known && (
                  <SelectItem value={value}>
                    {i18n._(msg`Unknown account`)}
                  </SelectItem>
                )}
              </SelectContent>
            </Select>
            <FormDescription className={CONFIG_DESCRIPTION}>
              {accounts && accounts.length === 0 ? (
                <Trans>
                  Add a signed-in BIGO account on the Network tab first.
                </Trans>
              ) : (
                <Trans>
                  The BIGO account that receives live notifications. Recording
                  never signs in to BIGO's app service with it, but uses its
                  cookies if it is also chosen on the Network tab.
                </Trans>
              )}
            </FormDescription>
          </FormItem>
        );
      }}
    />
  );
}

function listenerStateLabel(state: BigoNotificationStatus['state']): ReactNode {
  switch (state) {
    case 'disabled':
      return <Trans context="bigo notifications">Off</Trans>;
    case 'connecting':
      return <Trans context="bigo notifications">Connecting</Trans>;
    case 'connected':
      return <Trans context="bigo notifications">Connected</Trans>;
    case 'error':
      return <Trans context="bigo notifications">Not connected</Trans>;
    default:
      return state satisfies never;
  }
}

/** The saved listener's state, as the server reports it. */
function ListenerStatus({ status }: { status: BigoNotificationStatus }) {
  const { i18n } = useLingui();
  // Times relative to now wait for hydration, so they never differ from the server's render.
  const hydrated = useHydrated();
  const subscribed = status.subscribed;
  const total = status.total;
  const connected = status.state === 'connected';
  const missed = status.missed_pushes;
  const lastMissed =
    hydrated && status.last_missed_at
      ? formatRelativeTime(status.last_missed_at, i18n.locale)
      : null;
  // The status's websocket notice refreshes it once the attempt starts.
  const reconnect = useMutation({
    mutationFn: () => reconnectBigoNotifications(),
    onError: () => toast.error(i18n._(msg`Failed to reconnect`)),
  });
  // Waiting for another attempt, rather than for a settings change.
  const waiting =
    status.state === 'error' &&
    (status.reason === 'disconnected' || status.reason === 'signed_out');
  return (
    <div
      role="status"
      className="space-y-2 rounded-xl border border-border/50 bg-muted/20 p-3 text-xs"
    >
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="font-medium text-muted-foreground">
          <Trans>Live notifications</Trans>
        </span>
        <span
          className={
            connected
              ? 'font-semibold text-emerald-600 dark:text-emerald-400'
              : 'font-semibold text-amber-700 dark:text-amber-400'
          }
        >
          {listenerStateLabel(status.state)}
        </span>
      </div>
      {status.reason && (
        <p className="text-muted-foreground">
          {bigoReasonLabel(status.reason)}
        </p>
      )}
      {status.reason === 'notifications_not_arriving' && (
        <p className="text-amber-700 dark:text-amber-400">
          <Trans>
            Check that the notification account is not signed in to the BIGO app
            and that live notifications are on in its BIGO settings.
          </Trans>
        </p>
      )}
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="font-medium text-muted-foreground">
          <Trans>Subscribed streamers</Trans>
        </span>
        <span className="font-mono font-semibold">
          <Trans>
            {subscribed} of {total}
          </Trans>
        </span>
      </div>
      {missed > 0 && (
        <p className="text-amber-700 dark:text-amber-400">
          {lastMissed ? (
            <Trans>
              Starts BIGO did not notify: {missed}, the last {lastMissed}
            </Trans>
          ) : (
            <Trans>Starts BIGO did not notify: {missed}</Trans>
          )}
        </p>
      )}
      {status.message && (
        <div className="space-y-1">
          <p className="font-medium text-muted-foreground">
            <Trans>Last error</Trans>
          </p>
          <div className="break-all rounded-md border border-border/40 bg-muted/40 p-2 font-mono text-muted-foreground">
            {status.message}
          </div>
        </div>
      )}
      {waiting && (
        <Button
          type="button"
          variant="outline"
          size="sm"
          disabled={reconnect.isPending}
          onClick={() => reconnect.mutate()}
        >
          <RefreshCw className="mr-1.5 h-3.5 w-3.5" aria-hidden="true" />
          <Trans>Reconnect now</Trans>
        </Button>
      )}
    </div>
  );
}
