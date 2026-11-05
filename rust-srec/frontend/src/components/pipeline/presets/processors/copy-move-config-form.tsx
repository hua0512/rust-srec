import { Trans } from '@lingui/react/macro';
import {
  FormField,
  FormItem,
  FormControl,
  FormMessage,
  FormDescription,
} from '@/components/ui/form';
import { FlagFormField } from '@/components/ui/flag-form-field';
import { CONFIG_DESCRIPTION } from '@/components/config/shared/config-field';
import {
  ProcessorConfigFormProps,
  useProcessorFieldPath,
} from './common-props';
import {
  ProcessorFormShell,
  ProcessorSection,
  ProcessorSelectField,
  ProcessorTextField,
  TimeAnchorField,
} from './processor-form-layout';
import { CopyMoveConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { Copy, Settings2, Terminal } from 'lucide-react';
import { ListInput } from '@/components/ui/list-input';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { PLACEHOLDER_TOKENS } from '../../constants';

type CopyMoveConfig = z.infer<typeof CopyMoveConfigSchema>;

const OPERATIONS = [
  { value: 'copy', label: <Trans>Copy Files</Trans> },
  { value: 'move', label: <Trans>Move Files</Trans> },
];

export function CopyMoveConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps) {
  const { i18n } = useLingui();
  const fieldPath = useProcessorFieldPath<CopyMoveConfig>(pathPrefix);

  return (
    <ProcessorFormShell>
      <div className="space-y-6">
        <ProcessorSection
          icon={Copy}
          iconClassName="text-blue-500"
          title={<Trans>Operation</Trans>}
        >
          <div className="grid grid-cols-1 gap-6">
            <ProcessorSelectField
              control={control}
              name={fieldPath('operation')}
              label={<Trans>Operation Type</Trans>}
              placeholder={i18n._(msg`Select operation`)}
              options={OPERATIONS}
            />
            <ProcessorTextField
              control={control}
              name={fieldPath('destination')}
              label={<Trans>Destination Directory</Trans>}
              description={
                <Trans>
                  Supports placeholders: {PLACEHOLDER_TOKENS} and time tokens
                  like %Y/%m/%d.
                </Trans>
              }
              placeholder="/path/to/destination/{platform}/{streamer}/%Y-%m-%d"
              className="font-mono text-sm"
            />
            <TimeAnchorField
              control={control}
              name={fieldPath('time_anchor')}
              executionTime
              description={
                <Trans>
                  Use stream start time to keep a midnight-crossing stream in
                  one dated folder. Execution time preserves the existing
                  copy/move behavior.
                </Trans>
              }
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
              fieldName={fieldPath('create_dirs')}
              title={<Trans>Create Directories</Trans>}
              description={<Trans>Create missing folders</Trans>}
            />
            <FlagFormField
              fieldName={fieldPath('verify_integrity')}
              title={<Trans>Verify Integrity</Trans>}
              description={<Trans>Check size after copy</Trans>}
            />
            <FlagFormField
              fieldName={fieldPath('overwrite')}
              title={<Trans>Overwrite Existing</Trans>}
              description={
                <Trans>Replace destination files if they exist</Trans>
              }
            />
          </div>
        </ProcessorSection>

        <ProcessorSection
          icon={Terminal}
          iconClassName="text-orange-500"
          title={<Trans>Exclude Patterns</Trans>}
        >
          <FormField
            control={control}
            name={fieldPath('exclude_patterns')}
            render={({ field }) => (
              <FormItem>
                <FormControl>
                  <ListInput
                    value={field.value || []}
                    onChange={field.onChange}
                    placeholder={i18n._(msg`Add exclude regex pattern`)}
                  />
                </FormControl>
                <FormDescription className={CONFIG_DESCRIPTION}>
                  <Trans>
                    Regex patterns to exclude files. Matched against full path
                    and filename.
                  </Trans>
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
        </ProcessorSection>
      </div>
    </ProcessorFormShell>
  );
}
