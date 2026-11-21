import type { FieldValues, Path, UseFormReturn } from 'react-hook-form';
import { get } from 'react-hook-form';
import { Trans } from '@lingui/react/macro';
import { Radar } from 'lucide-react';
import { FlagFormField } from '@/components/ui/flag-form-field';
import { ConfigSectionHeading } from '@/components/config/shared/config-field';
import { configPath } from '@/components/config/shared/form-path';
import { useHasBackendCapability } from '@/hooks/use-backend-capabilities';

interface WeiboConfigFieldsProps<TFieldValues extends FieldValues> {
  form: UseFormReturn<TFieldValues>;
  /** Path to the object holding this platform's options. */
  fieldName: Path<TFieldValues>;
  inherited?: boolean;
}

export function WeiboConfigFields<TFieldValues extends FieldValues>({
  form,
  fieldName,
  inherited = false,
}: WeiboConfigFieldsProps<TFieldValues>) {
  const hasBatchDetection = useHasBackendCapability('weibo_batch_detection');

  return (
    <div className="space-y-12">
      <section className="space-y-6">
        <ConfigSectionHeading icon={Radar} accent="indigo">
          <Trans>Status Checks</Trans>
        </ConfigSectionHeading>
        <FlagFormField
          // Controllers retain their initial default when reset to undefined.
          // Recreate this field when saved defaults change to keep inheritance unset.
          key={
            inherited
              ? undefined
              : (JSON.stringify(
                  get(
                    form.formState.defaultValues,
                    `${fieldName}.batch_detection`,
                  ),
                ) ?? 'unset')
          }
          fieldName={configPath<TFieldValues>(fieldName, 'batch_detection')}
          inherited={inherited}
          disabled={!hasBatchDetection}
          showPremiumBadge
          title={<Trans>Batch Detection</Trans>}
          description={
            <>
              <Trans>
                Check monitored Weibo users together in one request using the
                public live list. Only covers public vertical app broadcasts.
              </Trans>
              {!hasBatchDetection && (
                <>
                  {' '}
                  <Trans>Requires a Premium backend.</Trans>
                </>
              )}
            </>
          }
          checked={(value) => !!value}
        />
      </section>
    </div>
  );
}
