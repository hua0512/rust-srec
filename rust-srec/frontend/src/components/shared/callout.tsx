import type { ComponentProps, ReactNode } from 'react';
import type { LucideIcon } from 'lucide-react';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { cn } from '@/lib/utils';

const CALLOUT_TONE = {
  info: 'border-blue-500/20 bg-blue-500/5 text-blue-600 dark:text-blue-400 *:data-[slot=alert-description]:text-blue-700/90 dark:*:data-[slot=alert-description]:text-blue-300/90',
  warning:
    'border-amber-500/30 bg-amber-500/5 text-amber-700 dark:text-amber-400 *:data-[slot=alert-description]:text-amber-700/90 dark:*:data-[slot=alert-description]:text-amber-400/90',
  error:
    'border-destructive/30 bg-destructive/5 text-destructive *:data-[slot=alert-description]:text-destructive/90',
} as const;

/**
 * A short tinted note beside a setting, or an error in a dialog or list.
 *
 * `Alert` announces itself as `role="alert"`; informational notes pass no role so screen readers
 * don't interrupt for them, and live status passes `status`. Errors keep `alert` unless told
 * otherwise. `action` sits under the text, such as a button that resolves the error.
 */
export function Callout({
  tone,
  icon: Icon,
  title,
  role = tone === 'error' ? 'alert' : undefined,
  action,
  children,
}: {
  tone: keyof typeof CALLOUT_TONE;
  icon: LucideIcon;
  title?: ReactNode;
  role?: ComponentProps<'div'>['role'];
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <Alert role={role} className={cn('rounded-xl py-2.5', CALLOUT_TONE[tone])}>
      <Icon />
      {title && <AlertTitle className="line-clamp-none">{title}</AlertTitle>}
      <AlertDescription className="text-xs leading-relaxed">
        <p>{children}</p>
        {action}
      </AlertDescription>
    </Alert>
  );
}
