import { motion, HTMLMotionProps } from 'motion/react';
import { LucideIcon } from 'lucide-react';
import React from 'react';

interface DashboardHeaderProps extends Omit<HTMLMotionProps<'div'>, 'title'> {
  icon: LucideIcon;
  title: React.ReactNode;
  subtitle?: React.ReactNode;
  actions?: React.ReactNode;
  children?: React.ReactNode;
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
  className,
  ...props
}: DashboardHeaderProps) {
  return (
    <motion.div
      className="border-b border-border/40"
      variants={item}
      {...props}
    >
      <div className="w-full">
        {/* Title Row */}
        <div className="flex flex-col md:flex-row gap-4 items-start md:items-center justify-between p-4 md:px-8">
          <div className="flex items-center gap-4">
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

          <div className="flex items-center gap-2 w-full md:w-auto overflow-x-auto no-scrollbar">
            {actions}
          </div>
        </div>

        {/* Extra Row (e.g. Filters) */}
        {children && (
          <div className="px-4 md:px-8 pb-3 overflow-x-auto no-scrollbar">
            {children}
          </div>
        )}
      </div>
    </motion.div>
  );
}
