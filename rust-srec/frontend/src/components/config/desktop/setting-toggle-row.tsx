import type { ReactNode } from 'react';

import { Switch } from '@/components/ui/switch';
import { cn } from '@/lib/utils';

interface SettingToggleRowProps {
  id: string;
  label: ReactNode;
  description: ReactNode;
  checked: boolean;
  disabled?: boolean;
  onCheckedChange: (checked: boolean) => void;
}

export function SettingToggleRow({
  id,
  label,
  description,
  checked,
  disabled = false,
  onCheckedChange,
}: SettingToggleRowProps) {
  const descriptionId = `${id}-description`;

  return (
    <div className="flex items-start justify-between gap-4 py-1">
      <div className="min-w-0 flex-1 space-y-1">
        <label
          htmlFor={id}
          className={cn(
            'block text-sm font-medium leading-snug text-foreground',
            disabled ? 'cursor-not-allowed opacity-70' : 'cursor-pointer',
          )}
        >
          {label}
        </label>
        <p
          id={descriptionId}
          className="text-xs leading-relaxed text-muted-foreground"
        >
          {description}
        </p>
      </div>
      <div className="flex shrink-0 items-center pt-0.5">
        <Switch
          id={id}
          checked={checked}
          disabled={disabled}
          aria-describedby={descriptionId}
          onCheckedChange={onCheckedChange}
        />
      </div>
    </div>
  );
}
