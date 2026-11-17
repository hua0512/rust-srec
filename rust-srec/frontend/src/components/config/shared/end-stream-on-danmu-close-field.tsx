import type { FieldValues, Path } from 'react-hook-form';
import { Trans } from '@lingui/react/macro';
import { FlagFormField } from '@/components/ui/flag-form-field';

interface EndStreamOnDanmuCloseFieldProps<TFieldValues extends FieldValues> {
  name: Path<TFieldValues>;
  inherited?: boolean;
}

export function EndStreamOnDanmuCloseField<TFieldValues extends FieldValues>({
  name,
  inherited = false,
}: EndStreamOnDanmuCloseFieldProps<TFieldValues>) {
  return (
    <FlagFormField
      fieldName={name}
      inherited={inherited}
      title={<Trans>End Stream On Danmu Close</Trans>}
      description={
        <Trans>Stop recording when danmu control signals stream closed.</Trans>
      }
      // Recording stops on a close signal unless this is `false`.
      checked={(value) => value !== false}
    />
  );
}
