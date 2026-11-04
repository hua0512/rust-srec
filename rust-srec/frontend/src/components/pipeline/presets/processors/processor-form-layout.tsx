import type { ComponentType, ReactNode } from 'react';
import { motion } from 'motion/react';
import { cn } from '@/lib/utils';

type IconComponent = ComponentType<{ className?: string }>;

const shellVariants = {
  hidden: { opacity: 0, y: 20 },
  visible: { opacity: 1, y: 0, transition: { duration: 0.3 } },
};

/** Fade-and-rise entrance shared by the processor config forms. */
export function ProcessorFormShell({ children }: { children: ReactNode }) {
  return (
    <motion.div
      variants={shellVariants}
      initial="hidden"
      animate="visible"
      className="w-full"
    >
      {children}
    </motion.div>
  );
}

/** Bordered group of fields under an icon heading. */
export function ProcessorSection({
  icon: Icon,
  iconClassName,
  title,
  actions,
  children,
}: {
  icon: IconComponent;
  /** Accent colour of the heading icon. */
  iconClassName?: string;
  title: ReactNode;
  /** Rendered at the trailing end of the heading row. */
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
      <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
        <Icon className={cn('w-4 h-4', iconClassName)} />
        <h3 className="font-semibold text-sm mr-auto">{title}</h3>
        {actions}
      </div>
      {children}
    </div>
  );
}

/**
 * Subtle subsection heading inside a Card. Used to group related form
 * fields without spawning extra Cards.
 */
export function ProcessorSubsectionHeader({
  icon: Icon,
  children,
}: {
  icon: IconComponent;
  children: ReactNode;
}) {
  return (
    <div className="flex items-center gap-2 pb-1">
      <Icon className="h-3.5 w-3.5 text-muted-foreground/70" />
      <h4 className="text-xs font-medium uppercase tracking-wider text-muted-foreground/80">
        {children}
      </h4>
    </div>
  );
}
