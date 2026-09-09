import * as React from 'react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Label } from '@/components/ui/label';

interface ColorPickerProps {
  label: string;
  cssVar: string;
  value: string;
  /**
   * Identity of the theme currently applied to the document (base, preset or
   * imported theme, and light/dark). Changing it re-reads the computed
   * variable, so the swatch follows the theme even while `value` stays empty.
   */
  themeKey: string;
  onChange: (cssVar: string, value: string) => void;
}

export function ColorPicker({
  label,
  cssVar,
  value,
  themeKey,
  onChange,
}: ColorPickerProps) {
  const [computedValue, setComputedValue] = React.useState('');

  // Read in a passive effect rather than during render: the server has no
  // computed styles, so a resolved colour in the first client render would not
  // match the server markup. Passive effects also flush after every layout
  // effect of the commit, which is where theme-provider swaps the light/dark
  // class on <html>; the user-theme <style> element is written synchronously
  // from the theme-settings store subscription, i.e. before the render this
  // effect belongs to. Both inputs are therefore settled by the time it runs.
  React.useEffect(() => {
    if (value) {
      setComputedValue('');
      return;
    }
    setComputedValue(
      getComputedStyle(document.documentElement)
        .getPropertyValue(cssVar)
        .trim(),
    );
  }, [cssVar, themeKey, value]);

  // An explicit override always wins over the theme's own value.
  const resolvedValue = value || computedValue;

  const handleColorChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newColor = e.target.value;
    onChange(cssVar, newColor);
  };

  const handleTextChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newValue = e.target.value;
    onChange(cssVar, newValue);
  };

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
