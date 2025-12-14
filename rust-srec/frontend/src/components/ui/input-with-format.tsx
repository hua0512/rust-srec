import * as React from 'react';
import { Input } from './input';
import { cn } from '../../lib/utils';
import { formatBytes, formatDuration } from '../../lib/format';

// Input does not export InputProps, but it uses React.ComponentProps<"input">
interface InputWithFormatProps extends React.ComponentProps<'input'> {
  formatType: 'size' | 'duration' | 'none';
}

const InputWithFormat = React.forwardRef<
  HTMLInputElement,
  InputWithFormatProps
>(({ className, formatType, value, onChange, ...props }, ref) => {
  const numericValue = Number(value || 0);

  let formattedDisplay = '';
  if (formatType === 'size') {
    formattedDisplay = formatBytes(numericValue);
  } else if (formatType === 'duration') {
    formattedDisplay =
      numericValue === 0 ? 'Unlimited' : formatDuration(numericValue);
  }

  return (
    <div className="relative">
      <Input
        type="number"
        className={cn('pr-24', className)}
        value={value}
        onChange={onChange}
        ref={ref}
        {...props}
      />
      {formatType !== 'none' && (
        <div className="absolute inset-y-0 right-0 flex items-center pr-3 pointer-events-none">
          <span className="text-xs text-muted-foreground bg-secondary/50 px-2 py-1 rounded">
            {formattedDisplay}
          </span>
        </div>
      )}
    </div>
  );
});
InputWithFormat.displayName = 'InputWithFormat';

export { InputWithFormat };
