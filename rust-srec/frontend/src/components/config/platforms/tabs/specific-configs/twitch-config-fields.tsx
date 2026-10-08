import type { FieldValues, Path, UseFormReturn } from 'react-hook-form';
import { Trans } from '@lingui/react/macro';
import { Shield } from 'lucide-react';
import { EndStreamOnDanmuCloseField } from '@/components/config/shared/end-stream-on-danmu-close-field';
import { ConfigSectionHeading } from '@/components/config/shared/config-field';
import { configPath } from '@/components/config/shared/form-path';

interface TwitchConfigFieldsProps<TFieldValues extends FieldValues> {
  form: UseFormReturn<TFieldValues>;
  /** Path to the object holding this platform's options. */
  fieldName: Path<TFieldValues>;
}

/** The OAuth token belongs to a Twitch account profile, not to these options. */
export function TwitchConfigFields<TFieldValues extends FieldValues>({
  form,
  fieldName,
}: TwitchConfigFieldsProps<TFieldValues>) {
  return (
    <section className="space-y-6">
      <ConfigSectionHeading icon={Shield} accent="indigo">
        <Trans>Danmu Control</Trans>
      </ConfigSectionHeading>
      <EndStreamOnDanmuCloseField
        form={form}
        name={configPath<TFieldValues>(
          fieldName,
          'end_stream_on_danmu_stream_closed',
        )}
      />
    </section>
  );
}
