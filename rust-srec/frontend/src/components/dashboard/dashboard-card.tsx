import { Card } from '@/components/ui/card';
import { cn } from '@/lib/utils';
import * as React from 'react';

interface DashboardCardProps extends React.ComponentProps<typeof Card> {
  children: React.ReactNode;
}

export function DashboardCard({
  className,
  children,
  ...props
}: DashboardCardProps) {
  return (
    <Card
      className={cn(
        'bg-white/60 dark:bg-card/40 backdrop-blur-xl border-black/5 dark:border-white/5 shadow-sm dark:shadow-2xl dark:shadow-black/5 hover:shadow-md dark:hover:shadow-black/10 transition-all duration-300 group overflow-hidden relative',
        className,
      )}
      {...props}
    >
      <div className="pointer-events-none absolute inset-0 bg-gradient-to-br from-primary/5 via-transparent to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-500" />
      {children}
    </Card>
  );
}
