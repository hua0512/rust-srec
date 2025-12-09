import { Control } from "react-hook-form"
import React from "react"
import { FormControl, FormDescription, FormField, FormItem, FormLabel } from "./form"
import { Badge } from "./badge"
import { Switch } from "./switch"

type FlagFormFieldProps = {
    controlPrefix?: string
    control: Control<any>
    fieldName: string
    title?: string | React.ReactNode
    description?: string | React.ReactNode
    ariaLabel?: string
    checked?: (value: any) => boolean
    onChange?: (value: boolean) => void
    showExperimentalBadge?: boolean
    children?: React.ReactNode
}

export function FlagFormField({
    controlPrefix,
    control,
    fieldName,
    title,
    description,
    ariaLabel,
    checked,
    onChange,
    showExperimentalBadge,
    children,
}: FlagFormFieldProps) {
    return (
        <FormField
            control={control}
            name={controlPrefix ? `${controlPrefix}.${fieldName}` : fieldName}
            render={({ field }) => (
                <FormItem className='flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm'>
                    <div className='space-y-1.5'>
                        <FormLabel>
                            <div className={"flex flex-row items-center gap-x-2"}>
                                {title}
                                {showExperimentalBadge && <Badge>Experimental</Badge>}
                            </div>
                        </FormLabel>
                        <FormDescription>{description}</FormDescription>
                        {children}
                    </div>
                    <FormControl>
                        <Switch
                            checked={checked ? checked(field.value) : field.value}
                            onCheckedChange={value => {
                                field.onChange(value)
                                if (onChange) {
                                    onChange(value)
                                }
                            }}
                            aria-label={ariaLabel}
                        />
                    </FormControl>
                </FormItem>
            )}
        />
    )
}
