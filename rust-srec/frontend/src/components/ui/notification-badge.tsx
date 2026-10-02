import type { ReactNode } from 'react';

import { cn } from '@/lib/utils';

interface NotificationBadgeProps {
  /** Pops the badge in when it turns true and collapses it when it turns false. */
  open: boolean;
  /** Also slide the badge in from the icon's centre; set only while it is turning on. */
  entering?: boolean;
  onEntered?: () => void;
  /** Positions the badge, which is absolutely placed over its icon. */
  className?: string;
  /** Size and colour of the popping element. */
  dotClassName?: string;
  children?: ReactNode;
}

/**
 * Corner badge over an icon, animated by the `.rs-notification-badge` rules
 * in styles.css (the transitions.dev "Notification badge" recipe). It stays
 * mounted while closed so the close transition can play; the badge is
 * decorative, so the owning control's label must carry what it says.
 */
export function NotificationBadge({
  open,
  entering,
  onEntered,
  className,
  dotClassName,
  children,
}: NotificationBadgeProps) {
  return (
    <span
      aria-hidden
      data-open={open}
      data-entering={entering || undefined}
      className={cn('rs-notification-badge absolute', className)}
      onAnimationEnd={(e) => {
        // Animations of the badge's children bubble up as well.
        if (e.target === e.currentTarget) onEntered?.();
      }}
    >
      <span
        className={cn(
          'rs-notification-badge-dot flex items-center justify-center',
          dotClassName,
        )}
      >
        {children}
      </span>
    </span>
  );
}
