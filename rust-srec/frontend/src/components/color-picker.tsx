import * as React from 'react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';

interface ColorPickerProps {
  label: string;
  cssVar: string;
  value: string;
  onChange: (cssVar: string, value: string) => void;
}

export function ColorPicker({
  label,
  cssVar,
  value,
  onChange,
}: ColorPickerProps) {
  const resolvedValue = React.useMemo(() => {
    if (value) return value;
    if (typeof window === 'undefined') return '';
    return getComputedStyle(document.documentElement)
      .getPropertyValue(cssVar)
      .trim();
  }, [cssVar, value]);

  const handleColorChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newColor = e.target.value;
    onChange(cssVar, newColor);
  };

  const handleTextChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newValue = e.target.value;
    onChange(cssVar, newValue);
  };

  // Get current computed color for display
  const swatchColor = resolvedValue || 'transparent';
  const colorInputValue =
    resolvedValue && resolvedValue.startsWith('#') ? resolvedValue : '#000000';

  return (
    <div className="space-y-2">
      <Label htmlFor={`color-${cssVar}`} className="text-xs font-medium">
        {label}
      </Label>
      <div className="flex items-start gap-2">
        <div className="relative">
          <Button
            type="button"
            variant="outline"
            className="h-8 w-8 p-0 overflow-hidden cursor-pointer"
            style={{ backgroundColor: swatchColor }}
          >
            <input
              type="color"
              id={`color-${cssVar}`}
              value={colorInputValue}
              onChange={handleColorChange}
              className="absolute inset-0 w-full h-full opacity-0 cursor-pointer"
            />
          </Button>
        </div>
        <Input
          type="text"
          placeholder={`${cssVar} value`}
          value={resolvedValue}
          onChange={handleTextChange}
          className="h-8 text-xs flex-1"
        />
      </div>
    </div>
  );
}
