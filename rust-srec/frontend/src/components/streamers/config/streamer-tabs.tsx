import { ReactNode } from 'react';
import { motion } from 'motion/react';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Badge } from '@/components/ui/badge';
import { itemVariants } from '@/lib/animation';
import { cn } from '@/lib/utils';

export interface StreamerTab {
  value: string;
  label: ReactNode;
  icon: ReactNode;
  /** Rendered next to the label, e.g. a filter count. */
  count?: number;
  content: ReactNode;
}

interface StreamerTabsProps {
  tabs: StreamerTab[];
  defaultValue?: string;
  className?: string;
}

/**
 * Column counts for the stacked mobile rail. Spelled out rather than interpolated so Tailwind's
 * scanner still sees each class.
 */
const GRID_COLS: Record<number, string> = {
  1: 'sm:grid-cols-1',
  2: 'sm:grid-cols-2',
  3: 'sm:grid-cols-3',
  4: 'sm:grid-cols-4',
};

/**
 * Tab shell for the streamer editor.
 *
 * Single source for the rail styling and the per-panel transition, which the create and edit
 * screens previously each carried their own copy of.
 */
export function StreamerTabs({
  tabs,
  defaultValue,
  className,
}: StreamerTabsProps) {
  return (
    <Tabs
      defaultValue={defaultValue ?? tabs[0]?.value}
      className={cn('w-full', className)}
    >
      <TabsList
        className={cn(
          // `self-start` matters: Tabs' root is `flex flex-col`, so without it this stretches
          // to full width and `md:w-auto` never takes effect.
          'grid h-auto w-full rounded-xl border bg-muted/30 p-1 backdrop-blur-sm md:inline-flex md:w-auto md:self-start md:rounded-full',
          GRID_COLS[tabs.length] ?? 'sm:grid-cols-3',
        )}
      >
        {tabs.map((tab) => (
          <TabsTrigger
            key={tab.value}
            value={tab.value}
            className="gap-2 rounded-lg px-4 py-2.5 transition-all data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm md:rounded-full md:px-6"
          >
            {tab.icon}
            {tab.label}
            {tab.count != null && tab.count > 0 && (
              <Badge
                variant="secondary"
                className="h-5 min-w-5 justify-center rounded-full bg-primary/10 px-1.5 text-[10px] font-bold text-primary"
              >
                {tab.count}
              </Badge>
            )}
          </TabsTrigger>
        ))}
      </TabsList>

      {tabs.map((tab) => (
        <TabsContent
          key={tab.value}
          value={tab.value}
          className="mt-6 border-none focus-visible:outline-none"
        >
          <motion.div
            variants={itemVariants}
            initial="hidden"
            animate="visible"
          >
            {tab.content}
          </motion.div>
        </TabsContent>
      ))}
    </Tabs>
  );
}
