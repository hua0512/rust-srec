import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { FlagFormField } from '@/components/ui/flag-form-field';
import {
  ProcessorConfigFormProps,
  useProcessorFieldPath,
} from './common-props';
import {
  ProcessorFormShell,
  ProcessorNumberField,
  ProcessorSection,
  ProcessorSelectField,
} from './processor-form-layout';
import { CompressionConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { Archive, Settings2 } from 'lucide-react';

type CompressionConfig = z.infer<typeof CompressionConfigSchema>;

const ARCHIVE_FORMATS = [
  { value: 'zip', label: 'ZIP' },
  { value: 'targz', label: 'TAR.GZ' },
];

export function CompressionConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps) {
  const { i18n } = useLingui();
  const fieldPath = useProcessorFieldPath<CompressionConfig>(pathPrefix);

  return (
    <ProcessorFormShell>
      <div className="space-y-6">
        <ProcessorSection
          icon={Archive}
          iconClassName="text-orange-500"
          title={<Trans>Archive Settings</Trans>}
        >
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <ProcessorSelectField
              control={control}
              name={fieldPath('format')}
              label={<Trans>Archive Format</Trans>}
              placeholder={i18n._(msg`Select format`)}
              options={ARCHIVE_FORMATS}
            />
            <ProcessorNumberField
              control={control}
              name={fieldPath('compression_level')}
              label={<Trans>Compression Level</Trans>}
              description={<Trans>0 (None) - 9 (Best)</Trans>}
              min={0}
              max={9}
              step={1}
            />
          </div>
        </ProcessorSection>

        <ProcessorSection
          icon={Settings2}
          iconClassName="text-gray-500"
          title={<Trans>Options</Trans>}
        >
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <FlagFormField
              fieldName={fieldPath('preserve_paths')}
              title={<Trans>Preserve Paths</Trans>}
              description={<Trans>Keep directory structure</Trans>}
            />
            <FlagFormField
              fieldName={fieldPath('overwrite')}
              title={<Trans>Overwrite</Trans>}
              description={<Trans>Replace output files</Trans>}
            />
          </div>
        </ProcessorSection>
      </div>
    </ProcessorFormShell>
  );
}
