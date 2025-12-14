import { formatBytes, formatDuration } from '../../lib/format';
import { cn } from '../../lib/utils';
import { Badge } from './badge';

interface FormattedValueProps extends React.HTMLAttributes<HTMLSpanElement> {
  value: number;
}

export function FormattedSize({
  value,
  className,
  ...props
}: FormattedValueProps) {
  return (
    <span
      className={cn('font-mono text-sm text-muted-foreground', className)}
      {...props}
    >
      {formatBytes(value)}
    </span>
  );
}

export function FormattedDuration({
  value,
  className,
  ...props
}: FormattedValueProps) {
  if (value === 0) {
    return (
      <span
        className={cn('font-mono text-sm text-muted-foreground', className)}
        {...props}
      >
        Unlimited
      </span>
    );
  }
  return (
    <span
      className={cn('font-mono text-sm text-muted-foreground', className)}
      {...props}
    >
      {formatDuration(value)}
    </span>
  );
}

interface FlagProps {
  value: boolean;
  trueLabel?: string;
  falseLabel?: string;
  className?: string;
}

export function Flag({
  value,
  trueLabel = 'Yes',
  falseLabel = 'No',
  className,
}: FlagProps) {
  return (
    <Badge
      variant={value ? 'default' : 'secondary'} // Or customized variants
      className={cn(
        'font-medium',
        value
          ? 'bg-green-500/15 text-green-700 hover:bg-green-500/25 border-green-200 dark:text-green-400 dark:border-green-800'
          : 'bg-red-500/15 text-red-700 hover:bg-red-500/25 border-red-200 dark:text-red-400 dark:border-red-800',
        'border px-2 py-0.5 shadow-none rounded-md',
        className,
      )}
    >
      {value ? trueLabel : falseLabel}
    </Badge>
  );
}
