import type { FieldValues, Path, UseFormReturn } from 'react-hook-form';
import { Trans } from '@lingui/react/macro';
import { Key } from 'lucide-react';
import { SecretFieldSection } from './secret-field-section';

interface TwitchConfigFieldsProps<TFieldValues extends FieldValues> {
  form: UseFormReturn<TFieldValues>;
  /** Path to the object holding this platform's options. */
  fieldName: Path<TFieldValues>;
}

export function TwitchConfigFields<TFieldValues extends FieldValues>({
  form,
  fieldName,
}: TwitchConfigFieldsProps<TFieldValues>) {
  return (
    <SecretFieldSection
      form={form}
      fieldName={fieldName}
      optionKey="oauth_token"
      icon={Key}
      heading={<Trans>Authentication</Trans>}
      label={<Trans>OAuth Token</Trans>}
      placeholder="oauth:..."
      autoComplete="off"
      description={
        <Trans>
          Twitch OAuth token for subscriber-only and high-quality streams.
        </Trans>
      }
    />
  );
}
