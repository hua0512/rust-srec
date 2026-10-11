import { motion, HTMLMotionProps } from 'motion/react';
import { LucideIcon } from 'lucide-react';
import React from 'react';

import { cn } from '@/lib/utils';

interface DashboardHeaderProps extends Omit<HTMLMotionProps<'div'>, 'title'> {
  icon: LucideIcon;
  title: React.ReactNode;
  subtitle?: React.ReactNode;
  actions?: React.ReactNode;
  children?: React.ReactNode;
  /** Classes for the row around `children`, which scrolls horizontally by default. */
  childrenClassName?: string;
}

const item = {
  hidden: { opacity: 0, y: 20 },
  show: { opacity: 1, y: 0 },
};

export function DashboardHeader({
  icon: Icon,
  title,
  subtitle,
  actions,
  children,
  childrenClassName,
  ...props
}: DashboardHeaderProps) {
  return (
    <motion.div
      className="border-b border-border/40"
      variants={item}
      {...props}
    >
      <div className="w-full">
        {/* Title Row: the actions share the title's row while both fit in the width the page
            is given and wrap below it otherwise. A viewport breakpoint cannot tell, since the
            sidebar takes part of the viewport. */}
        <div className="flex flex-wrap gap-4 items-center justify-between p-4 md:px-8">
          <div className="flex min-w-0 flex-1 basis-64 items-center gap-4">
            <div className="p-2.5 rounded-xl bg-gradient-to-br from-primary/20 to-primary/5 ring-1 ring-primary/10">
              <Icon className="h-6 w-6 text-primary" />
            </div>
            <div>
              <h1 className="text-xl font-semibold tracking-tight">{title}</h1>
              {subtitle && (
                <p className="text-sm text-muted-foreground">{subtitle}</p>
              )}
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-2 w-full sm:w-auto">
            {actions}
          </div>
        </div>

        {/* Extra Row (e.g. Filters) */}
        {children && (
          <div
            className={cn(
              'px-4 md:px-8 pb-3 overflow-x-auto no-scrollbar',
              childrenClassName,
            )}
          >
            {children}
          </div>
        )}
      </div>
    </motion.div>
  );
}
