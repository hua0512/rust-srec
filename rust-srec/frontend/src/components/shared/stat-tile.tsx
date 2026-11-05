import type { ReactNode } from 'react';
import { motion } from 'motion/react';
import { itemVariants } from '@/lib/animation';
import { cn } from '@/lib/utils';

/**
 * One figure in a row of figures. Inside a parent animating through
 * `containerVariants` it staggers in with its siblings.
 */
export function StatTile({
  icon,
  label,
  value,
  accent,
  hint,
  className,
}: {
  /** A small icon, coloured to match `accent`. */
  icon: ReactNode;
  label: ReactNode;
  value: ReactNode;
  /** Background of the icon tile and the hover wash, e.g. `bg-blue-500/10`. */
  accent: string;
  hint?: ReactNode;
  className?: string;
}) {
  return (
    <motion.div
      variants={itemVariants}
      className={cn(
        'group/tile relative overflow-hidden rounded-xl border border-border/50 bg-background/40 p-3 hover:bg-background/60 hover:border-border transition-colors',
        className,
      )}
    >
      <div
        className={cn(
          'absolute inset-0 opacity-0 group-hover/tile:opacity-100 transition-opacity duration-300',
          accent,
        )}
      />
      <div className="relative">
        <div className="mb-1.5 flex items-center gap-1.5 text-[11px] font-medium uppercase tracking-wider text-muted-foreground">
          <div
            className={cn(
              'h-6 w-6 rounded-md flex items-center justify-center shrink-0',
              accent,
            )}
          >
            {icon}
          </div>
          <span className="min-w-0 truncate">{label}</span>
        </div>
        <p className="font-mono text-lg font-bold tracking-tight text-foreground">
          {value}
        </p>
        {hint && (
          <p className="mt-0.5 font-mono text-[11px] text-muted-foreground">
            {hint}
          </p>
        )}
      </div>
    </motion.div>
  );
}
