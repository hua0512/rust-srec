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

export type UnitType = 'size' | 'duration';

interface UnitOption {
  label: string;
  value: number;
}

const SIZE_UNITS: UnitOption[] = [
  { label: 'Bytes', value: 1 },
  { label: 'KB', value: 1024 },
  { label: 'MB', value: 1024 * 1024 },
  { label: 'GB', value: 1024 * 1024 * 1024 },
  { label: 'TB', value: 1024 * 1024 * 1024 * 1024 },
];

const DURATION_UNITS: UnitOption[] = [
  { label: 'Secs', value: 1 },
  { label: 'Mins', value: 60 },
  { label: 'Hours', value: 3600 },
  { label: 'Days', value: 86400 },
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
  const units = getUnits(unitType);

  // If value is null/undefined, treat it as null (empty input)
  // If it's 0, it's 0.
  const isNullValue = value === null || value === undefined;
  const safeValue = isNullValue ? 0 : Number(value);

  const [unitMultiplier, setUnitMultiplier] = React.useState<number>(1);
  const lastEmittedValue = React.useRef<number | null | undefined>(undefined);

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
    if (!isNullValue && safeValue > 0) {
      const currentUnits = getUnits(unitType);
      let bestUnit = 1;
      for (let i = currentUnits.length - 1; i >= 0; i--) {
        if (safeValue >= currentUnits[i].value) {
          bestUnit = currentUnits[i].value;
          break;
        }
      }
      setUnitMultiplier(bestUnit);
    } else if (!isNullValue && safeValue === 0) {
      setUnitMultiplier(1);
    }
    // If null, we typically keep the unit as is or default to 1, but we don't display it anyway if empty?
    // Actually we do display unit select always. default to 1 is fine.
  }, [value, safeValue, unitType, isNullValue]);

  // Calculate the display value based on current unit
  // If null, displayValue is null (mapped to empty string in input)
  const displayValue = isNullValue ? '' : safeValue / unitMultiplier;

  const handleInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const valStr = e.target.value;
    if (valStr === '') {
      onChange(null);
      lastEmittedValue.current = null;
      return;
    }

    const newVal = e.target.valueAsNumber;
    if (isNaN(newVal)) {
      // Invalid number input, maybe don't emit change or emit null?
      // Browser type="number" usually handles this by returning empty string if invalid.
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

    if (isNullValue) return;

    // When changing unit, preserve the NUMBER in the input
    const currentInputNumber = Number(displayValue);
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
        'flex items-center rounded-md border border-input bg-transparent shadow-sm ring-offset-background focus-within:ring-1 focus-within:ring-ring focus-within:border-primary',
        className,
      )}
      {...props}
    >
      <Input
        type="number"
        className={cn(
          'flex-1 border-0 shadow-none focus-visible:ring-0 focus-visible:ring-offset-0 bg-transparent rounded-r-none h-9',
          inputClassName,
        )}
        value={displayValue}
        onChange={handleInputChange}
        placeholder={placeholder}
        min={min}
        max={max}
        step={step}
      />
      <div className="h-4 w-[1px] bg-border shrink-0" />
      <Select value={currentUnitValue} onValueChange={handleUnitChange}>
        <SelectTrigger className="w-[85px] border-0 shadow-none focus:ring-0 focus:ring-offset-0 rounded-l-none h-9 px-3 gap-1 hover:bg-muted/50">
          <SelectValue placeholder="Unit" />
        </SelectTrigger>
        <SelectContent align="end">
          {units.map((u) => (
            <SelectItem key={u.label} value={u.value.toString()}>
              {u.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}
