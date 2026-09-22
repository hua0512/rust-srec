import { Control, FieldValues } from 'react-hook-form';

export interface ProcessorConfigFormProps<T extends FieldValues> {
  control: Control<T>;
  pathPrefix?: string;
}
