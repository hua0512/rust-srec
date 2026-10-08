import { useId, useRef, useState, type ReactNode } from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import {
  AlertCircle,
  AlertTriangle,
  Cookie,
  Globe,
  Info,
  KeyRound,
  Lock,
  Network,
  QrCode,
  RefreshCw,
  Smartphone,
  User,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { FormControl, FormDescription, FormItem } from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { SwitchCard } from '@/components/ui/switch-card';
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs';
import {
  CONFIG_DESCRIPTION,
  CONFIG_INPUT,
  ConfigFieldLabel,
} from '@/components/config/shared/config-field';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { cn } from '@/lib/utils';
import { formatPlatformName } from '@/lib/format';
import {
  createCredentialProfile,
  updateCredentialProfile,
} from '@/server/functions/credential-profiles';
import type { CredentialProfileDetail } from '@/api/schemas/credential-profiles';
import { INHERIT_ROUTE, type ProxyRoute } from '@/api/schemas/proxies';
import { ProxyRoutePicker } from '@/components/config/shared/proxy-route-picker';
import { sameRoute } from '@/components/proxies/proxy-route-label';
import { Callout } from '@/components/shared/callout';
import { platformCapabilitiesQueryOptions } from '@/api/credential-profiles';
import { errorBody, errorDetails, hasErrorCode } from '@/lib/api-error';

/** An account that inherits follows the route of the recording using it. */
const FOLLOW_RECORDING = { kind: 'recording' } as const;

/** How a new account signs in: a QR code scanned in the app, or pasted credentials. */
type SignIn = 'qr' | 'paste';

/** The sites typed into the editor, separated by commas, spaces or lines. */
function parseSites(text: string): string[] {
  return text
    .split(/[\s,]+/)
    .map((site) => site.trim())
    .filter(Boolean);
}

/** Why saving failed, naming the account that already has a site. */
function SaveError({ error }: { error: Error }) {
  const details = errorDetails(error);
  if (
    hasErrorCode(errorBody(error), 'CREDENTIAL_SITE_TAKEN') &&
    typeof details?.site === 'string' &&
    typeof details.label === 'string'
  ) {
    const site = details.site;
    const label = details.label;
    return (
      <Trans>
        {site} already belongs to the account {label}. A site can belong to one
        account only.
      </Trans>
    );
  }
  return <>{error.message}</>;
}

/**
 * Adds an account to the platform, or edits one's label, its proxy setting
 * and optionally its credentials. Saved immediately, independently of the
 * configuration form. A new account on a platform with QR login offers it
 * first; the QR login then creates the account with the label and proxy
 * setting chosen here.
 */
export function ProfileEditorDialog({
  platformId,
  platformName,
  profile,
  replace: initialReplace = false,
  onClose,
  onSaved,
  onQr,
}: {
  platformId: string;
  /** The platform whose app scans a QR code. */
  platformName?: string;
  profile?: CredentialProfileDetail;
  /** Open an edit with credential replacement switched on. */
  replace?: boolean;
  onClose: () => void;
  onSaved: () => void;
  onQr: (label: string, proxyRoute: ProxyRoute) => void;
}) {
  const [label, setLabel] = useState(profile?.profile.label ?? '');
  const labelInput = useRef<HTMLInputElement>(null);
  // A new account always needs material; an edit replaces it only on request.
  const [replace, setReplace] = useState(!profile || initialReplace);
  const [chosenSignIn, setSignIn] = useState<SignIn>();
  // Material replacement always starts empty; summaries never contain recoverable secrets.
  const [cookies, setCookies] = useState('');
  const [refreshToken, setRefreshToken] = useState('');
  const [accessToken, setAccessToken] = useState('');
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const storedRoute: ProxyRoute = profile?.profile.proxy_route ?? INHERIT_ROUTE;
  const [proxyRoute, setProxyRoute] = useState<ProxyRoute>(storedRoute);
  const routeChanged = !sameRoute(proxyRoute, storedRoute);
  const proxyId = useId();
  const storedSites = profile?.sites ?? [];
  const [sitesText, setSitesText] = useState(storedSites.join(', '));
  const sites = parseSites(sitesText);
  const sitesChanged = sites.join(',') !== storedSites.join(',');
  const sitesId = useId();
  // Until the platform's rules load, only cookies are offered.
  const fields = useQuery(platformCapabilitiesQueryOptions(platformId)).data;
  // Streamlink accounts name the sites they are for.
  const namesSites = fields?.per_streamer_selection ?? false;
  // QR login is the default for a new account wherever the platform has it.
  const signIn: SignIn =
    !profile && fields?.qr_login ? (chosenSignIn ?? 'qr') : 'paste';
  const appName = platformName ? formatPlatformName(platformName) : undefined;
  const save = useMutation({
    mutationFn: async () => {
      const material = {
        cookies,
        refresh_token: refreshToken || null,
        access_token: accessToken || null,
        reauth_config:
          fields?.reauth_login && (username || password)
            ? { username, password }
            : null,
      };
      if (profile)
        await updateCredentialProfile({
          data: {
            id: profile.profile.id,
            expected_version: profile.profile.version,
            label,
            ...(replace ? { replacement: material } : {}),
            ...(routeChanged ? { proxy_route: proxyRoute } : {}),
            ...(namesSites && sitesChanged ? { sites } : {}),
          },
        });
      else
        await createCredentialProfile({
          data: {
            platform_id: platformId,
            label,
            enabled: true,
            material,
            ...(proxyRoute.kind !== 'inherit'
              ? { proxy_route: proxyRoute }
              : {}),
            ...(namesSites && sites.length ? { sites } : {}),
          },
        });
    },
    onSuccess: onSaved,
  });
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <DialogContent
        className="max-h-[90vh] overflow-y-auto border-border/50 sm:max-w-lg"
        // Radix would select the label's text; place the caret instead.
        onOpenAutoFocus={(event) => {
          const input = labelInput.current;
          if (!input) return;
          event.preventDefault();
          input.focus();
          input.setSelectionRange(input.value.length, input.value.length);
        }}
      >
        <DialogHeader>
          <DialogTitle>
            {profile ? <Trans>Edit account</Trans> : <Trans>Add account</Trans>}
          </DialogTitle>
          <DialogDescription>
            <Trans>
              Saved right away, separately from the rest of this page.
            </Trans>
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-4">
          <FormItem className="space-y-2">
            <ConfigFieldLabel>
              <Trans>Label</Trans>
            </ConfigFieldLabel>
            <FormControl>
              <Input
                ref={labelInput}
                className={CONFIG_INPUT}
                value={label}
                maxLength={128}
                onChange={(event) => setLabel(event.target.value)}
              />
            </FormControl>
          </FormItem>
          {profile && (
            <SwitchCard
              label={<Trans>Replace credentials</Trans>}
              description={
                <Trans>Leave off to keep the stored credentials.</Trans>
              }
              checked={replace}
              onCheckedChange={setReplace}
            />
          )}
          {!profile && fields?.qr_login && (
            <Tabs
              value={signIn}
              onValueChange={(value) => setSignIn(value as SignIn)}
            >
              <TabsList className="h-10 w-full rounded-xl">
                <TabsTrigger value="qr" className="rounded-lg">
                  <QrCode />
                  <Trans>Scan QR code</Trans>
                </TabsTrigger>
                <TabsTrigger value="paste" className="rounded-lg">
                  <Cookie />
                  <Trans>Paste cookies</Trans>
                </TabsTrigger>
              </TabsList>
            </Tabs>
          )}
          {signIn === 'qr' && (
            <Callout tone="info" icon={Smartphone}>
              {appName ? (
                <Trans>
                  Sign in by scanning a QR code with the {appName} app. Nothing
                  to copy or paste.
                </Trans>
              ) : (
                <Trans>
                  Sign in by scanning a QR code with the platform&apos;s app.
                  Nothing to copy or paste.
                </Trans>
              )}
            </Callout>
          )}
          {replace && signIn === 'paste' && (
            <div className="@container space-y-4">
              <Callout
                tone={profile ? 'warning' : 'info'}
                icon={profile ? AlertTriangle : Info}
              >
                {profile ? (
                  <Trans>
                    Enter the new credentials in full. Token and login fields
                    left empty are cleared. Saved credentials are never shown.
                  </Trans>
                ) : (
                  <Trans>
                    Enter the credentials of this one account. Never mix cookies
                    from different accounts.
                  </Trans>
                )}
              </Callout>
              <FormItem className="space-y-2">
                <ConfigFieldLabel icon={Cookie}>
                  <Trans>Cookies</Trans>
                </ConfigFieldLabel>
                <FormControl>
                  <Textarea
                    autoComplete="off"
                    className={SECRET_TEXTAREA}
                    value={cookies}
                    onChange={(event) => setCookies(event.target.value)}
                  />
                </FormControl>
              </FormItem>
              {fields?.refresh_token && (
                <SecretField
                  icon={RefreshCw}
                  label={<Trans>Refresh token</Trans>}
                  value={refreshToken}
                  onChange={setRefreshToken}
                />
              )}
              {fields?.access_token && (
                <SecretField
                  icon={KeyRound}
                  label={<Trans>Access token</Trans>}
                  value={accessToken}
                  onChange={setAccessToken}
                  description={
                    fields.token_only && (
                      <Trans>
                        The access token alone signs in; cookies are optional.
                      </Trans>
                    )
                  }
                />
              )}
              {fields?.reauth_login && (
                <div className="grid grid-cols-1 gap-4 @md:grid-cols-2">
                  <FormItem className="space-y-2">
                    <ConfigFieldLabel icon={User}>
                      <Trans>Username</Trans>
                    </ConfigFieldLabel>
                    <FormControl>
                      <Input
                        autoComplete="off"
                        className={CONFIG_INPUT}
                        value={username}
                        onChange={(event) => setUsername(event.target.value)}
                      />
                    </FormControl>
                  </FormItem>
                  <SecretField
                    icon={Lock}
                    label={<Trans>Password</Trans>}
                    value={password}
                    onChange={setPassword}
                  />
                </div>
              )}
            </div>
          )}
          {namesSites && (
            <div className="space-y-2">
              <ConfigFieldLabel icon={Globe} plain>
                <label htmlFor={sitesId}>
                  <Trans>Sites</Trans>
                </label>
              </ConfigFieldLabel>
              <Input
                id={sitesId}
                className={CONFIG_INPUT}
                placeholder="youtube.com, kick.com"
                autoComplete="off"
                spellCheck={false}
                value={sitesText}
                onChange={(event) => setSitesText(event.target.value)}
              />
              <p className={CONFIG_DESCRIPTION}>
                <Trans>
                  Streamers on these sites that don&apos;t choose an account use
                  this one. A site also covers its subdomains, so youtube.com
                  covers www.youtube.com and m.youtube.com. Separate sites with
                  commas.
                </Trans>
              </p>
            </div>
          )}
          <div className="space-y-2">
            <ConfigFieldLabel icon={Network} plain>
              <label htmlFor={proxyId}>
                <Trans>Proxy</Trans>
              </label>
            </ConfigFieldLabel>
            <ProxyRoutePicker
              id={proxyId}
              value={proxyRoute}
              onChange={setProxyRoute}
              inherit={FOLLOW_RECORDING}
            />
            <p className={CONFIG_DESCRIPTION}>
              <Trans>
                Account checks, refreshes, recordings, danmu and playback with
                this account go through this setting. Following the recording
                uses the proxy of whatever uses the account; checks then use the
                platform&apos;s.
              </Trans>
            </p>
          </div>
          {save.error && (
            <Callout tone="error" icon={AlertCircle}>
              <SaveError error={save.error} />
            </Callout>
          )}
        </div>
        <DialogFooter>
          {signIn === 'qr' ? (
            <Button
              type="button"
              disabled={!label.trim()}
              onClick={() => onQr(label.trim(), proxyRoute)}
            >
              <QrCode className="h-4 w-4" />
              <Trans>Show QR code</Trans>
            </Button>
          ) : (
            <Button
              type="button"
              disabled={save.isPending || !label.trim()}
              onClick={() => save.mutate()}
            >
              <Trans>Save account</Trans>
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

const SECRET_TEXTAREA =
  'min-h-24 rounded-xl border-border/50 bg-background/50 font-mono text-xs shadow-sm transition-all focus:bg-background';

/** A masked credential input; stored values are never shown, so it always starts empty. */
function SecretField({
  icon,
  label,
  value,
  onChange,
  description,
}: {
  icon: LucideIcon;
  label: ReactNode;
  value: string;
  onChange: (value: string) => void;
  description?: ReactNode;
}) {
  return (
    <FormItem className="space-y-2">
      <ConfigFieldLabel icon={icon}>{label}</ConfigFieldLabel>
      <FormControl>
        <Input
          type="password"
          autoComplete="new-password"
          className={cn(CONFIG_INPUT, 'font-mono text-xs')}
          value={value}
          onChange={(event) => onChange(event.target.value)}
        />
      </FormControl>
      {description && (
        <FormDescription className={CONFIG_DESCRIPTION}>
          {description}
        </FormDescription>
      )}
    </FormItem>
  );
}
