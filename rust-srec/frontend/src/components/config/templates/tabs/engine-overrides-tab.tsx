import { useState } from 'react';
import { UseFormReturn } from 'react-hook-form';
import { useQuery } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { Plus, Server } from 'lucide-react';
import { listEngines } from '@/server/functions';
import { EngineOverrideCard } from './engine-override-card';
import { AnimatePresence, motion } from 'motion/react';
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from '@/components/ui/command';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/components/ui/popover';
import { Button } from '@/components/ui/button';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  CardDescription,
} from '@/components/ui/card';

interface EngineOverridesTabProps {
  form: UseFormReturn<any>;
}

export function EngineOverridesTab({ form }: EngineOverridesTabProps) {
  const { i18n } = useLingui();
  const [open, setOpen] = useState(false);

  // Fetch available engines
  const { data: engines = [] } = useQuery({
    queryKey: ['engines'],
    queryFn: () => listEngines(),
  });

  const currentOverrides = form.watch('engines_override') || {};
  const overriddenIds = Object.keys(currentOverrides);

  // Filter out engines that are already overridden
  const availableEngines = engines.filter((e) => !overriddenIds.includes(e.id));

  const handleAddOverride = (engineId: string) => {
    const engine = engines.find((e) => e.id === engineId);
    if (!engine) return;

    const newOverrides = { ...currentOverrides, [engineId]: {} };
    form.setValue('engines_override', newOverrides, { shouldDirty: true });
    setOpen(false);
  };

  const handleRemoveOverride = (engineId: string) => {
    const newOverrides = { ...currentOverrides };
    delete newOverrides[engineId];
    form.setValue('engines_override', newOverrides, { shouldDirty: true });
  };

  return (
    <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
      <CardHeader className="pb-3">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-lg bg-indigo-500/10 text-indigo-600 dark:text-indigo-400">
              <Server className="w-5 h-5" />
            </div>
            <div className="space-y-1">
              <CardTitle className="text-lg">
                <Trans>Engine Overrides</Trans>
              </CardTitle>
              <CardDescription>
                <Trans>
                  Customize specific engine settings for this template.
                </Trans>
              </CardDescription>
            </div>
          </div>

          <Popover open={open} onOpenChange={setOpen}>
            <PopoverTrigger asChild>
              <Button variant="outline" size="sm" className="gap-2">
                <Plus className="h-4 w-4" />
                <Trans>Add Override</Trans>
              </Button>
            </PopoverTrigger>
            <PopoverContent className="w-[300px] p-0" align="end">
              <Command>
                <CommandInput placeholder={i18n._(msg`Search engines...`)} />
                <CommandList>
                  <CommandEmpty>
                    <Trans>No engines found.</Trans>
                  </CommandEmpty>
                  <CommandGroup>
                    {availableEngines.map((engine) => (
                      <CommandItem
                        key={engine.id}
                        value={engine.name}
                        onSelect={() => handleAddOverride(engine.id)}
                      >
                        <div className="flex flex-col">
                          <span>{engine.name}</span>
                          <span className="text-xs text-muted-foreground">
                            {engine.engine_type}
                          </span>
                        </div>
                      </CommandItem>
                    ))}
                  </CommandGroup>
                </CommandList>
              </Command>
            </PopoverContent>
          </Popover>
        </div>
      </CardHeader>

      <CardContent className="space-y-4">
        {overriddenIds.length === 0 ? (
          <div className="flex flex-col items-center justify-center p-8 border-2 border-dashed rounded-lg text-muted-foreground bg-muted/10">
            <Server className="w-10 h-10 mb-3 opacity-20" />
            <p className="text-sm font-medium">
              <Trans>No overrides configured</Trans>
            </p>
            <p className="text-xs mt-1 max-w-xs text-center">
              <Trans>
                Add an override to customize engine behavior for this template.
              </Trans>
            </p>
          </div>
        ) : (
          <div className="space-y-4">
            <AnimatePresence mode="popLayout">
              {overriddenIds.map((engineId) => {
                const engine = engines.find((e) => e.id === engineId);
                if (!engine) {
                  return (
                    <motion.div
                      key={engineId}
                      layout
                      initial={{ opacity: 0, scale: 0.95 }}
                      animate={{ opacity: 1, scale: 1 }}
                      exit={{ opacity: 0, scale: 0.95 }}
                      transition={{ duration: 0.2 }}
                    >
                      <div className="flex items-center justify-between p-4 border rounded bg-destructive/10 text-destructive">
                        <span>
                          <Trans>Unknown Engine ID: {engineId}</Trans>
                        </span>
                        <Button
                          variant="ghost"
                          size="sm"
                          onClick={() => handleRemoveOverride(engineId)}
                        >
                          <Trans>Remove</Trans>
                        </Button>
                      </div>
                    </motion.div>
                  );
                }

                return (
                  <motion.div
                    key={engineId}
                    layout
                    initial={{ opacity: 0, scale: 0.95 }}
                    animate={{ opacity: 1, scale: 1 }}
                    exit={{ opacity: 0, scale: 0.95 }}
                    transition={{ duration: 0.2 }}
                  >
                    <EngineOverrideCard
                      engineId={engineId}
                      engineName={engine.name}
                      engineType={engine.engine_type}
                      form={form}
                      onRemove={() => handleRemoveOverride(engineId)}
                    />
                  </motion.div>
                );
              })}
            </AnimatePresence>
          </div>
        )}
      </CardContent>
    </Card>
  );
}
