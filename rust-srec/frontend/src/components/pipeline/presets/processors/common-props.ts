import { Control, FieldValues, UseFormRegister } from "react-hook-form";

export interface ProcessorConfigFormProps<T extends FieldValues> {
    control: Control<T>;
    register?: UseFormRegister<T>;
    pathPrefix?: string;
}
