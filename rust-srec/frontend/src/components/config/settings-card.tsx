import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import { cn } from '@/lib/utils';
import { LucideIcon } from 'lucide-react';
import { ReactNode } from 'react';

interface SettingsCardProps {
  title: ReactNode;
  description: ReactNode;
  icon: LucideIcon;
  iconColor?: string;
  iconBgColor?: string;
  children: ReactNode;
  className?: string;
  action?: ReactNode;
}

export function SettingsCard({
  title,
  description,
  icon: Icon,
  iconColor = 'text-primary',
  iconBgColor = 'bg-primary/10',
  children,
  className,
  action,
}: SettingsCardProps) {
  return (
    <Card
      className={cn(
        'h-full border-white/10 bg-background/30 backdrop-blur-xl shadow-xl hover:shadow-2xl transition-all duration-300 hover:bg-background/40 hover:scale-[1.01] group',
        className,
      )}
    >
      <CardHeader className="space-y-1">
        <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4">
          <div className="flex items-start sm:items-center gap-3">
            <div
              className={cn(
                'p-2.5 rounded-xl ring-1 ring-inset ring-black/5 dark:ring-white/5 transition-colors duration-300 group-hover:scale-105 shrink-0',
                iconBgColor,
              )}
            >
              <Icon className={cn('w-5 h-5', iconColor)} />
            </div>
            <div className="space-y-1">
              <CardTitle className="text-lg font-semibold tracking-tight leading-none">
                {title}
              </CardTitle>
              <CardDescription className="text-sm text-muted-foreground/80 leading-snug">
                {description}
              </CardDescription>
            </div>
          </div>
          {action && <div className="self-end sm:self-auto">{action}</div>}
        </div>
      </CardHeader>
      <CardContent className="pt-2">{children}</CardContent>
    </Card>
  );
}
