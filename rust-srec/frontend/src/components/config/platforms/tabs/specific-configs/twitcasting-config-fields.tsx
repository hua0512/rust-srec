import type { FieldValues, Path, UseFormReturn } from 'react-hook-form';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Trans } from '@lingui/react/macro';
import { Lock } from 'lucide-react';
import { SecretFieldSection } from './secret-field-section';

interface TwitcastingConfigFieldsProps<TFieldValues extends FieldValues> {
  form: UseFormReturn<TFieldValues>;
  /** Path to the object holding this platform's options. */
  fieldName: Path<TFieldValues>;
}

export function TwitcastingConfigFields<TFieldValues extends FieldValues>({
  form,
  fieldName,
}: TwitcastingConfigFieldsProps<TFieldValues>) {
  const { i18n } = useLingui();
  return (
    <SecretFieldSection
      form={form}
      fieldName={fieldName}
      optionKey="password"
      icon={Lock}
      heading={<Trans>Protection Settings</Trans>}
      label={<Trans>Stream Password</Trans>}
      placeholder={i18n._(msg`Password...`)}
      description={
        <Trans>
          Required if the stream is password-protected by the broadcaster.
        </Trans>
      }
    />
  );
}
