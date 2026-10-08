import {
  get,
  type FieldValues,
  type Path,
  type UseFormReturn,
} from 'react-hook-form';
import { Trans } from '@lingui/react/macro';
import { FormField, FormItem, FormMessage } from '@/components/ui/form';
import {
  CredentialSelectionSchema,
  type CredentialOwner,
  type CredentialPlatform,
  type CredentialSelection,
} from '@/api/schemas/credential-profiles';
import { AccountSelectionSection } from './account-selection-section';
import { PlatformAccountsSection } from './platform-accounts-section';
import { ReadOnlyAccountsSection } from './read-only-accounts-section';

/** A missing selection inherits, like an explicit `inherit`. */
function sameSelection(
  a: CredentialSelection | undefined,
  b: CredentialSelection | undefined,
): boolean {
  const left = a ?? { mode: 'inherit' };
  const right = b ?? { mode: 'inherit' };
  switch (left.mode) {
    case 'inherit':
    case 'none':
      return right.mode === left.mode;
    case 'fixed':
      return (
        right.mode === 'fixed' && right.credential_id === left.credential_id
      );
    default:
      return (
        right.mode === 'pool' &&
        right.strategy === left.strategy &&
        right.failover === left.failover &&
        right.max_attempts === left.max_attempts &&
        right.credential_ids.length === left.credential_ids.length &&
        right.credential_ids.every(
          (id, index) => id === left.credential_ids[index],
        )
      );
  }
}

function parseSelection(value: unknown): CredentialSelection | undefined {
  const parsed = CredentialSelectionSchema.safeParse(value);
  return parsed.success ? parsed.data : undefined;
}

/**
 * The account settings of one configuration scope, as one panel: its
 * selection first, then the platform's accounts. The platform page manages
 * them there; templates and streamers list them read-only.
 */
export function CredentialSettings({
  scope,
  platform,
  selection,
  onSelectionChange,
  dirty,
}: {
  scope: CredentialOwner;
  platform: CredentialPlatform;
  selection?: CredentialSelection;
  onSelectionChange: (selection: CredentialSelection) => void;
  /** The form holds a selection that is not saved yet. */
  dirty?: boolean;
}) {
  const selectionSection = (
    <AccountSelectionSection
      scope={scope}
      platformId={platform.id}
      selection={selection}
      onSelectionChange={onSelectionChange}
      dirty={dirty}
    />
  );
  return scope.type === 'platform' ? (
    <PlatformAccountsSection
      platformId={platform.id}
      platformName={platform.name}
      selection={selectionSection}
    />
  ) : (
    <ReadOnlyAccountsSection
      platformId={platform.id}
      selection={selectionSection}
    />
  );
}

/**
 * Binds a scope's account selection to its configuration form.
 *
 * `scope` is absent until the template or streamer is saved, and `platform`
 * until the caller has resolved which platform the selection applies to.
 */
export function CredentialSettingsField<T extends FieldValues>({
  form,
  name,
  scope,
  platform,
}: {
  form: UseFormReturn<T>;
  name: Path<T>;
  scope?: CredentialOwner;
  platform?: CredentialPlatform;
}) {
  if (!scope)
    return (
      <p className="text-sm text-muted-foreground">
        <Trans>
          Save this template or streamer before choosing its accounts.
        </Trans>
      </p>
    );
  if (!platform)
    return (
      <p className="text-sm text-muted-foreground">
        <Trans>Choose a single platform to manage its accounts.</Trans>
      </p>
    );
  return (
    <FormField
      control={form.control}
      name={name}
      render={({ field }) => {
        const selection = parseSelection(field.value);
        // Compared with what the form was loaded or last reset with, which is
        // what Save Changes would replace.
        const saved = parseSelection(get(form.formState.defaultValues, name));
        return (
          <FormItem>
            <CredentialSettings
              scope={scope}
              platform={platform}
              selection={selection}
              onSelectionChange={field.onChange}
              dirty={!sameSelection(selection, saved)}
            />
            <FormMessage />
          </FormItem>
        );
      }}
    />
  );
}
