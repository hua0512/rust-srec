import { ReactNode } from 'react';
import { cn } from '@/lib/utils';

type TooltipColorTheme =
  | 'amber'
  | 'orange'
  | 'red'
  | 'violet'
  | 'blue'
  | 'slate';

interface StatusInfoTooltipProps {
  icon: ReactNode;
  title: ReactNode;
  subtitle?: ReactNode;
  theme: TooltipColorTheme;
  children: ReactNode;
  className?: string;
}

const themeStyles: Record<
  TooltipColorTheme,
  {
    headerGradient: string;
    headerBorder: string;
    iconBg: string;
    iconColor: string;
    ring: string;
    shadow: string;
  }
> = {
  amber: {
    headerGradient:
      'from-amber-500/10 via-amber-500/5 to-transparent border-amber-500/10',
    headerBorder: 'border-amber-500/10',
    iconBg: 'bg-amber-500/10',
    iconColor: 'text-amber-600',
    ring: 'ring-amber-500/20',
    shadow: 'shadow-[inset_0_0_10px_rgba(245,158,11,0.1)]',
  },
  orange: {
    headerGradient:
      'from-orange-500/10 via-orange-500/5 to-transparent border-orange-500/10',
    headerBorder: 'border-orange-500/10',
    iconBg: 'bg-orange-500/10',
    iconColor: 'text-orange-600',
    ring: 'ring-orange-500/20',
    shadow: 'shadow-[inset_0_0_10px_rgba(249,115,22,0.1)]',
  },
  red: {
    headerGradient:
      'from-red-500/10 via-red-500/5 to-transparent border-red-500/10',
    headerBorder: 'border-red-500/10',
    iconBg: 'bg-red-500/10',
    iconColor: 'text-red-600',
    ring: 'ring-red-500/20',
    shadow: 'shadow-[inset_0_0_10px_rgba(239,68,68,0.1)]',
  },
  violet: {
    headerGradient:
      'from-violet-500/10 via-violet-500/5 to-transparent border-violet-500/10',
    headerBorder: 'border-violet-500/10',
    iconBg: 'bg-violet-500/10',
    iconColor: 'text-violet-600',
    ring: 'ring-violet-500/20',
    shadow: 'shadow-[inset_0_0_10px_rgba(139,92,246,0.1)]',
  },
  blue: {
    headerGradient:
      'from-blue-500/10 via-blue-500/5 to-transparent border-blue-500/10',
    headerBorder: 'border-blue-500/10',
    iconBg: 'bg-blue-500/10',
    iconColor: 'text-blue-600',
    ring: 'ring-blue-500/20',
    shadow: 'shadow-[inset_0_0_10px_rgba(59,130,246,0.1)]',
  },
  slate: {
    headerGradient:
      'from-slate-500/10 via-slate-500/5 to-transparent border-slate-500/10',
    headerBorder: 'border-slate-500/10',
    iconBg: 'bg-slate-500/10',
    iconColor: 'text-slate-600 dark:text-slate-400',
    ring: 'ring-slate-500/20',
    shadow: 'shadow-[inset_0_0_10px_rgba(100,116,139,0.1)]',
  },
};

export function StatusInfoTooltip({
  icon,
  title,
  subtitle,
  theme,
  children,
  className,
}: StatusInfoTooltipProps) {
  const styles = themeStyles[theme];

  // Extract color for CSS variables
  const colorMatch = styles.iconColor.match(/text-([a-z]+)-600/);
  const colorName = colorMatch ? colorMatch[1] : 'primary';

  return (
    <div
      className={cn('flex flex-col min-w-[280px] max-w-[340px]', className)}
      style={
        {
          '--tooltip-theme-color': `var(--${colorName}-500, currentColor)`,
          '--tooltip-theme-bg': `var(--${colorName}-500-10, color-mix(in srgb, currentColor 10%, transparent))`,
        } as React.CSSProperties
      }
    >
      {/* Header */}
      <div
        className={cn('p-3 bg-gradient-to-br border-b', styles.headerGradient)}
      >
        <div className="flex items-center gap-2.5">
          <div
            className={cn(
              'p-1.5 rounded-full ring-1 transition-colors duration-300',
              styles.iconBg,
              styles.iconColor,
              styles.shadow,
              styles.ring,
            )}
          >
            {icon}
          </div>
          <div>
            <p className="font-semibold text-sm leading-none tracking-tight text-foreground/90">
              {title}
            </p>
            {subtitle && (
              <p className="text-[10px] text-muted-foreground mt-1 font-medium">
                {subtitle}
              </p>
            )}
          </div>
        </div>
      </div>

      {/* Content */}
      <div className="p-3 space-y-3 group/tooltip-content">{children}</div>
    </div>
  );
}
