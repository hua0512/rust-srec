import { ReactNode } from 'react';
import { cn } from '@/lib/utils';

interface GraphViewportProps {
  children: ReactNode;
  className?: string;
}

export function GraphViewport({ children, className }: GraphViewportProps) {
  return (
    <div
      className={cn(
        'relative w-full h-full overflow-hidden bg-muted/30 backdrop-blur-2xl rounded-2xl border border-border/80 shadow-2xl group/graph',
        className,
      )}
    >
      {/* Background Mesh/Blobs */}
      <div className="absolute inset-0 pointer-events-none overflow-hidden opacity-50">
        <div className="absolute top-0 right-0 w-64 h-64 bg-primary/15 rounded-full blur-[80px]" />
        <div className="absolute bottom-0 left-0 w-80 h-80 bg-blue-500/10 rounded-full blur-[100px]" />
        <div className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 w-96 h-96 bg-purple-500/5 rounded-full blur-[120px]" />
      </div>

      {/* Background Grid */}
      <div
        className="absolute inset-0 opacity-[0.03] pointer-events-none"
        style={{
          backgroundImage: `radial-gradient(circle at 1px 1px, currentColor 1px, transparent 0)`,
          backgroundSize: '40px 40px',
        }}
      />

      {children}
    </div>
  );
}

interface GlassNodeProps {
  children: ReactNode;
  className?: string;
  glow?: string;
  bg?: string;
  isClickable?: boolean;
}

export function GlassNode({
  children,
  className,
  glow = 'shadow-transparent',
  bg = 'bg-muted/5',
  isClickable,
}: GlassNodeProps) {
  return (
    <div
      className={cn(
        'group relative p-5 rounded-2xl border transition-all duration-500 hover:scale-[1.05] hover:-translate-y-2',
        'bg-card backdrop-blur-3xl shadow-xl',
        'border-border/60 hover:border-primary/40',
        glow,
        isClickable && 'cursor-pointer',
        className,
      )}
    >
      {/* Status Background Accent */}
      <div
        className={cn(
          'absolute inset-0 rounded-2xl opacity-[0.03] group-hover:opacity-[0.08] transition-opacity duration-500',
          bg,
        )}
      />

      {/* Glass reflection */}
      <div className="absolute inset-0 rounded-2xl bg-gradient-to-br from-white/10 to-transparent pointer-events-none" />

      <div className="relative z-10">{children}</div>
    </div>
  );
}
