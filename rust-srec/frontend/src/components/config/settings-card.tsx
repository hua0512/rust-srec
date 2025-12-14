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
        'h-full border-border/50 bg-card/50 backdrop-blur-sm shadow-sm hover:shadow-md transition-all duration-300 hover:border-border/80 group',
        className,
      )}
    >
      <CardHeader className="space-y-1">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div
              className={cn(
                'p-2.5 rounded-xl ring-1 ring-inset ring-black/5 dark:ring-white/5 transition-colors duration-300 group-hover:scale-105',
                iconBgColor,
              )}
            >
              <Icon className={cn('w-5 h-5', iconColor)} />
            </div>
            <div>
              <CardTitle className="text-lg font-semibold tracking-tight">
                {title}
              </CardTitle>
              <CardDescription className="text-sm text-muted-foreground/80 mt-1">
                {description}
              </CardDescription>
            </div>
          </div>
          {action && <div>{action}</div>}
        </div>
      </CardHeader>
      <CardContent className="pt-2">{children}</CardContent>
    </Card>
  );
}
