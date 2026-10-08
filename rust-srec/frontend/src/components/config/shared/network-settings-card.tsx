import { CredentialSettingsField } from '@/components/credentials/credential-settings-field';
import type { FieldValues, Path, UseFormReturn } from 'react-hook-form';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import { Trans } from '@lingui/react/macro';
import { Cookie, Network } from 'lucide-react';
import { RetryPolicyForm } from './retry-policy-form';
import type {
  CredentialOwner,
  CredentialPlatform,
} from '@/api/schemas/credential-profiles';
import { genericMemo } from '@/lib/generic-component';

interface NetworkSettingsCardProps<TFieldValues extends FieldValues> {
  form: UseFormReturn<TFieldValues>;
  paths: {
    /** Absent where accounts are chosen elsewhere, e.g. a template's per-platform overrides. */
    credentialSelection?: Path<TFieldValues>;
    retryPolicy: Path<TFieldValues>;
  };
  configMode?: 'json' | 'object';
  /**
   * The saved configuration whose selection the form edits; absent until it is
   * saved. Must keep its identity between renders; this card is memoized.
   */
  credentialScope?: CredentialOwner;
  /** Must keep its identity between renders; this card is memoized. */
  credentialPlatform?: CredentialPlatform;
}

function NetworkSettingsCardImpl<TFieldValues extends FieldValues>({
  form,
  paths,
  configMode = 'object',
  credentialScope,
  credentialPlatform,
}: NetworkSettingsCardProps<TFieldValues>) {
  return (
    <div className="grid gap-6">
      {/* Authentication Card */}
      <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
        <CardHeader className="pb-3 px-4 pt-6 sm:px-6">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-lg bg-orange-500/10 text-orange-600 dark:text-orange-400">
              <Cookie className="w-5 h-5" />
            </div>
            <div>
              <CardTitle className="text-lg">
                <Trans>Authentication</Trans>
              </CardTitle>
              <CardDescription>
                <Trans>Choose which accounts sign in to the platform.</Trans>
              </CardDescription>
            </div>
          </div>
        </CardHeader>
        <CardContent className="px-4 pb-6 space-y-6 sm:px-6">
          {paths.credentialSelection ? (
            <CredentialSettingsField
              form={form}
              name={paths.credentialSelection}
              scope={credentialScope}
              platform={credentialPlatform}
            />
          ) : (
            <p className="text-sm text-muted-foreground">
              <Trans>
                Accounts are chosen per platform. Set them in this
                template&apos;s platform overrides.
              </Trans>
            </p>
          )}
        </CardContent>
      </Card>

      {/* Retry Policy Card */}
      <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
        <CardHeader className="pb-3">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-lg bg-green-500/10 text-green-600 dark:text-green-400">
              <Network className="w-5 h-5" />
            </div>
            <CardTitle className="text-lg">
              <Trans>Download Retry Policy</Trans>
            </CardTitle>
          </div>
        </CardHeader>
        <CardContent>
          <RetryPolicyForm
            form={form}
            name={paths.retryPolicy}
            mode={configMode}
          />
        </CardContent>
      </Card>
    </div>
  );
}

export const NetworkSettingsCard = genericMemo(
  NetworkSettingsCardImpl,
  'NetworkSettingsCard',
);
