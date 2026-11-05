import { FlagFormField } from '@/components/ui/flag-form-field';
import {
  ProcessorConfigFormProps,
  useProcessorFieldPath,
} from './common-props';
import {
  ProcessorFormShell,
  ProcessorListField,
  ProcessorSection,
  ProcessorTextField,
} from './processor-form-layout';
import { DanmakuFactoryConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { Settings, Terminal, Trash2 } from 'lucide-react';

type DanmakuFactoryConfig = z.infer<typeof DanmakuFactoryConfigSchema>;

const INPUT_TOKEN = '{input}';
const OUTPUT_TOKEN = '{output}';

export function DanmakuFactoryConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps) {
  const { i18n } = useLingui();
  const fieldPath = useProcessorFieldPath<DanmakuFactoryConfig>(pathPrefix);

  return (
    <ProcessorFormShell>
      <div className="space-y-6">
        <ProcessorSection
          icon={Terminal}
          iconClassName="text-blue-500"
          title={<Trans>Execution Settings</Trans>}
        >
          <div className="grid grid-cols-1 gap-6">
            <ProcessorTextField
              control={control}
              name={fieldPath('binary_path')}
              label={<Trans>Binary Path</Trans>}
              description={
                <Trans>
                  Path to DanmakuFactory binary. If empty, uses environment
                  variable or PATH.
                </Trans>
              }
              placeholder="DanmakuFactory"
              className="font-mono text-sm"
            />
            <ProcessorListField
              control={control}
              name={fieldPath('args')}
              label={<Trans>Command Arguments</Trans>}
              // Tokens are bound values: literal `{...}` inlined into a lingui message becomes
              // an unbound ICU placeholder and renders as an empty string.
              description={
                <Trans>
                  Command template args. Use {INPUT_TOKEN} and {OUTPUT_TOKEN}{' '}
                  placeholders.
                </Trans>
              }
              placeholder={i18n._(msg`Add argument template`)}
            />
            <ProcessorListField
              control={control}
              name={fieldPath('extra_args')}
              label={<Trans>Extra Arguments</Trans>}
              description={
                <Trans>Additional arguments appended to the command.</Trans>
              }
              placeholder={i18n._(msg`Add extra argument`)}
            />
          </div>
        </ProcessorSection>

        <ProcessorSection
          icon={Settings}
          iconClassName="text-orange-500"
          title={<Trans>Processing Behavior</Trans>}
        >
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FlagFormField
              fieldName={fieldPath('overwrite')}
              title={<Trans>Overwrite Output</Trans>}
              description={<Trans>Overwrite existing .ass files</Trans>}
            />
            <FlagFormField
              fieldName={fieldPath('verify_output_exists')}
              title={<Trans>Verify Output</Trans>}
              description={<Trans>Check if file exists after conversion</Trans>}
            />
            <FlagFormField
              fieldName={fieldPath('prefer_manifest')}
              title={<Trans>Prefer session pairing</Trans>}
              description={
                <Trans>
                  Convert only the danmu recorded for this session's segments
                </Trans>
              }
            />
            <FlagFormField
              fieldName={fieldPath('passthrough_inputs')}
              title={<Trans>Passthrough Inputs</Trans>}
              description={<Trans>Include original files in outputs</Trans>}
            />
          </div>
        </ProcessorSection>

        <ProcessorSection
          icon={Trash2}
          iconClassName="text-red-500"
          title={<Trans>Cleanup</Trans>}
        >
          <FlagFormField
            fieldName={fieldPath('delete_source_xml_on_success')}
            title={<Trans>Delete Source XML</Trans>}
            description={
              <Trans>Delete XML files after successful conversion</Trans>
            }
          />
        </ProcessorSection>
      </div>
    </ProcessorFormShell>
  );
}
