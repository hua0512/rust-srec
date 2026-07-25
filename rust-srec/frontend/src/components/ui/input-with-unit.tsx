import * as React from 'react';
import { Input } from './input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from './select';
import { cn } from '../../lib/utils';
import type { MessageDescriptor } from '@lingui/core';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';

export type UnitType = 'size' | 'duration';

interface UnitOption {
  /**
   * Message descriptor rather than a string: these live at module scope, so they cannot be
   * resolved until a component renders and has the active locale.
   */
  label: MessageDescriptor;
  value: number;
}

const SIZE_UNITS: UnitOption[] = [
  { label: msg`Bytes`, value: 1 },
  { label: msg`KB`, value: 1024 },
  { label: msg`MB`, value: 1024 * 1024 },
  { label: msg`GB`, value: 1024 * 1024 * 1024 },
  { label: msg`TB`, value: 1024 * 1024 * 1024 * 1024 },
];

const DURATION_UNITS: UnitOption[] = [
  { label: msg`ms`, value: 0.001 },
  { label: msg`Secs`, value: 1 },
  { label: msg`Mins`, value: 60 },
  { label: msg`Hours`, value: 3600 },
  { label: msg`Days`, value: 86400 },
];

function getUnits(unitType: UnitType): UnitOption[] {
  return unitType === 'size' ? SIZE_UNITS : DURATION_UNITS;
}

interface InputWithUnitProps extends Omit<
  React.ComponentProps<'div'>,
  'onChange'
> {
  value: number | null | undefined;
  onChange: (value: number | null) => void;
  unitType: UnitType;
  inputClassName?: string;
  placeholder?: string;
  min?: number;
  max?: number;
  step?: number;
}

export function InputWithUnit({
  value,
  onChange,
  unitType,
  className,
  inputClassName,
  placeholder,
  min,
  max,
  step,
  ...props
}: InputWithUnitProps) {
  const { i18n } = useLingui();
  const units = getUnits(unitType);

  // If value is null/undefined, treat it as null (empty input)
  // If it's 0, it's 0.
  const isNullValue = value === null || value === undefined;
  const safeValue = isNullValue ? 0 : Number(value);

  const [unitMultiplier, setUnitMultiplier] = React.useState<number>(1);
  const lastEmittedValue = React.useRef<number | null | undefined>(undefined);
  const [inputValue, setInputValue] = React.useState<string>(
    isNullValue ? '' : (safeValue / unitMultiplier).toString(),
  );

  // Auto-convert unit when value changes externally
  React.useEffect(() => {
    // If the value matches what we last emitted, it's a loopback from our own change.
    if (
      lastEmittedValue.current !== undefined &&
      value === lastEmittedValue.current
    ) {
      return;
    }

    // External update (load, reset, etc.)
    if (!isNullValue) {
      if (safeValue > 0) {
        const currentUnits = getUnits(unitType);
        let bestUnit = 1;
        for (let i = currentUnits.length - 1; i >= 0; i--) {
          if (safeValue >= currentUnits[i].value) {
            bestUnit = currentUnits[i].value;
            break;
          }
        }
        setUnitMultiplier(bestUnit);
        setInputValue((safeValue / bestUnit).toString());
      } else if (safeValue === 0) {
        setUnitMultiplier(1);
        setInputValue('0');
      }
    } else {
      setInputValue('');
    }
  }, [value, safeValue, unitType, isNullValue]);

  const handleInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const valStr = e.target.value;
    setInputValue(valStr);

    if (valStr === '') {
      onChange(null);
      lastEmittedValue.current = null;
      return;
    }

    const newVal = e.target.valueAsNumber;
    if (isNaN(newVal)) {
      return;
    }

    const computedValue = newVal * unitMultiplier;
    onChange(computedValue);
    lastEmittedValue.current = computedValue;
  };

  const handleUnitChange = (newUnitValStr: string) => {
    const newUnitVal = Number(newUnitValStr);
    if (!newUnitVal || newUnitVal <= 0) return;

    setUnitMultiplier(newUnitVal);

    // When changing unit, preserve the NUMBER in the input
    const currentInputNumber = Number(inputValue);
    if (isNaN(currentInputNumber)) return;

    const computedValue = currentInputNumber * newUnitVal;
    onChange(computedValue);
    lastEmittedValue.current = computedValue;
  };

  const currentUnitValue = units.some((u) => u.value === unitMultiplier)
    ? unitMultiplier.toString()
    : units[0].value.toString();

  return (
    <div
      className={cn(
        'flex h-11 items-center rounded-xl border border-input bg-transparent shadow-sm ring-offset-background focus-within:border-primary focus-within:ring-1 focus-within:ring-ring',
        className,
      )}
      {...props}
    >
      <Input
        type="number"
        className={cn(
          'h-full min-w-0 flex-1 rounded-r-none border-0 bg-transparent shadow-none focus-visible:ring-0 focus-visible:ring-offset-0',
          inputClassName,
        )}
        value={inputValue}
        onChange={handleInputChange}
        placeholder={placeholder}
        min={min}
        max={max}
        step={step ?? 'any'}
      />
      <div className="h-4 w-[1px] bg-border shrink-0" />
      <Select value={currentUnitValue} onValueChange={handleUnitChange}>
        <SelectTrigger className="h-full w-auto shrink-0 gap-1 rounded-l-none border-0 px-2.5 shadow-none hover:bg-muted/50 focus:ring-0 focus:ring-offset-0 data-[size=default]:h-full">
          <SelectValue placeholder={i18n._(msg`Unit`)} />
        </SelectTrigger>
        <SelectContent align="end">
          {units.map((u) => (
            <SelectItem key={u.value} value={u.value.toString()}>
              {i18n._(u.label)}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}
