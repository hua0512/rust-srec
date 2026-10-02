import type {
  FieldValues,
  Path,
  PathValue,
  UseFormReturn,
} from 'react-hook-form';
import { useQuery } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import { listPlatformConfigs } from '@/server/functions/config';
import { getStreamer } from '@/server/functions/streamers';
import type { CredentialSaveScope } from '@/server/functions/credentials';
import { FormField, FormItem, FormMessage } from '@/components/ui/form';
import {
  CredentialSelectionSchema,
  type CredentialOwner,
} from '@/api/schemas/credential-profiles';
import { CredentialProfilesPanel } from './credential-profiles-panel';

export function CredentialSettingsField<T extends FieldValues>({
  form,
  name,
  scope,
  platformName,
}: {
  form: UseFormReturn<T>;
  name: Path<T>;
  scope?: CredentialSaveScope | null;
  platformName?: string;
}) {
  const { data: platforms = [] } = useQuery({
    queryKey: ['config', 'platforms'],
    queryFn: () => listPlatformConfigs(),
    enabled: Boolean(scope),
  });
  const streamer = useQuery({
    queryKey: ['streamer', scope?.type === 'streamer' ? scope.id : null],
    queryFn: () => getStreamer({ data: scope?.id ?? '' }),
    enabled: scope?.type === 'streamer',
  });
  const matches = platforms.filter((platform) =>
    scope?.type === 'platform'
      ? platform.id === scope.id
      : scope?.type === 'streamer'
        ? platform.id === streamer.data?.platform_config_id
        : platform.name === platformName,
  );
  const platform = matches.length === 1 ? matches[0] : undefined;
  if (!scope)
    return (
      <p className="text-sm text-muted-foreground">
        <Trans>
          Save this template or streamer before adding local account profiles.
        </Trans>
      </p>
    );
  if (!platform)
    return (
      <p className="text-sm text-muted-foreground">
        <Trans>
          Select an unambiguous platform to manage account profiles.
        </Trans>
      </p>
    );
  const owner: CredentialOwner =
    scope.type === 'platform'
      ? { type: 'platform', platform_id: scope.id }
      : scope.type === 'template'
        ? { type: 'template', template_id: scope.id }
        : { type: 'streamer', streamer_id: scope.id };
  return (
    <FormField
      control={form.control}
      name={name}
      render={({ field }) => {
        const parsed = CredentialSelectionSchema.safeParse(field.value);
        const onConverted = (value: unknown) => {
          // Conversion has already cleared these server-owned inputs. Preserve unrelated unsaved
          // form changes while preventing a subsequent save from echoing stale legacy material.
          const clear = (fields: Record<string, unknown>) => {
            const result = { ...fields };
            if (
              platform.name.toLowerCase() === 'bigo' &&
              typeof result.stream_password !== 'string' &&
              typeof result.password === 'string'
            )
              result.stream_password = result.password;
            for (const key of [
              'cookies',
              'refresh_token',
              'access_token',
              'oauth_token',
              'ttwid',
              'device_id',
              'username',
              'password',
              'session_cookies',
              'last_cookie_check_date',
              'last_cookie_check_result',
            ])
              delete result[key];
            if (
              result.platform_specific_config &&
              typeof result.platform_specific_config === 'object'
            )
              result.platform_specific_config = clear(
                result.platform_specific_config as Record<string, unknown>,
              );
            if (
              result.platform_extras &&
              typeof result.platform_extras === 'object'
            )
              result.platform_extras = clear(
                result.platform_extras as Record<string, unknown>,
              );
            return result;
          };
          if (scope.type === 'platform') {
            form.setValue('cookies' as Path<T>, null as PathValue<T, Path<T>>);
            const path = 'platform_specific_config' as Path<T>;
            const specific = form.getValues(path);
            if (specific && typeof specific === 'object')
              form.setValue(path, clear(specific) as PathValue<T, Path<T>>);
          } else {
            const path = name.slice(0, name.lastIndexOf('.')) as Path<T>;
            const fields = form.getValues(path);
            if (fields && typeof fields === 'object')
              form.setValue(path, {
                ...clear(fields),
                credential_selection: value,
              } as PathValue<T, Path<T>>);
          }
          field.onChange(value);
        };
        return (
          <FormItem>
            <CredentialProfilesPanel
              owner={owner}
              platformId={platform.id}
              platformName={platform.name}
              selection={parsed.success ? parsed.data : undefined}
              onSelectionChange={field.onChange}
              onConverted={onConverted}
            />
            <FormMessage />
          </FormItem>
        );
      }}
    />
  );
}
