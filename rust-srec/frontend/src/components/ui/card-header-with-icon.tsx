import * as React from 'react';
import { CardHeader, CardTitle } from './card';
import { cn } from '@/lib/utils';
import { LucideIcon } from 'lucide-react';

export interface CardHeaderWithIconProps {
  icon: LucideIcon;
  title: React.ReactNode;
  description?: React.ReactNode;
  iconClassName?: string;
  iconBgClassName?: string;
  className?: string;
  children?: React.ReactNode;
}

/**
 * Card header with an icon badge and title.
 * Reduces duplication of the "flex items-center gap-2 + icon in rounded bg + title" pattern.
 */
function CardHeaderWithIcon({
  icon: Icon,
  title,
  description,
  iconClassName,
  iconBgClassName,
  className,
  children,
}: CardHeaderWithIconProps) {
  return (
    <CardHeader className={cn('pb-3', className)}>
      <div className="flex items-center gap-2">
        <div className={cn('p-2 bg-primary/10 rounded-md', iconBgClassName)}>
          <Icon className={cn('h-5 w-5 text-primary', iconClassName)} />
        </div>
        <div className="flex-1">
          <CardTitle className="text-base font-medium">{title}</CardTitle>
          {description && (
            <p className="text-sm text-muted-foreground">{description}</p>
          )}
        </div>
        {children}
      </div>
    </CardHeader>
  );
}

export { CardHeaderWithIcon };
