import * as React from 'react';
import { Input } from './input';
import { cn } from '@/lib/utils';
import { LucideIcon } from 'lucide-react';

export interface IconInputProps extends React.ComponentProps<'input'> {
  icon: LucideIcon;
  iconPosition?: 'left' | 'right';
}

/**
 * Input component with an icon positioned inside the input field.
 * Reduces duplication of the "relative div + absolute icon + padded input" pattern.
 */
const IconInput = React.forwardRef<HTMLInputElement, IconInputProps>(
  ({ className, icon: Icon, iconPosition = 'left', ...props }, ref) => {
    const isLeft = iconPosition === 'left';

    return (
      <div className="relative">
        <Icon
          className={cn(
            // Centred rather than pinned to a fixed offset, so the icon stays put
            // whatever height the caller gives the input.
            'pointer-events-none absolute top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground',
            isLeft ? 'left-2.5' : 'right-2.5',
          )}
        />
        <Input
          ref={ref}
          className={cn(isLeft ? 'pl-9' : 'pr-9', className)}
          {...props}
        />
      </div>
    );
  },
);
IconInput.displayName = 'IconInput';

export { IconInput };
