import { UseFormReturn } from 'react-hook-form';
import { useQuery } from '@tanstack/react-query';
import { listPlatformConfigs, listEngines } from '@/server/functions';
import { Button } from '@/components/ui/button';
import { Plus, LayoutGrid } from 'lucide-react';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from '@/components/ui/command';
import { useState } from 'react';
import { Trans } from '@lingui/react/macro';

import { PlatformOverrideCard } from './platform-override-card';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  CardDescription,
} from '@/components/ui/card';

interface PlatformOverridesTabProps {
  form: UseFormReturn<any>;
}

export function PlatformOverridesTab({ form }: PlatformOverridesTabProps) {
  const [open, setOpen] = useState(false);

  // Let's use `listPlatforms` which returns `PlatformConfigSchema[]`.
  const { data: platforms = [] } = useQuery({
    queryKey: ['config', 'platforms'],
    queryFn: () => listPlatformConfigs(),
  });

  const { data: engines = [] } = useQuery({
    queryKey: ['engines'],
    queryFn: () => listEngines(),
  });

  const currentOverrides = form.watch('platform_overrides') || {};

  const handleAddOverride = (platformName: string) => {
    // platformName matches the name in the map
    const newOverrides = { ...currentOverrides, [platformName]: {} };
    form.setValue('platform_overrides', newOverrides, { shouldDirty: true });
    setOpen(false);
  };

  const handleRemoveOverride = (platformName: string) => {
    const newOverrides = { ...currentOverrides };
    delete newOverrides[platformName];
    form.setValue('platform_overrides', newOverrides, { shouldDirty: true });
  };

  // Filter out already overridden platforms
  const availablePlatforms = platforms.filter(
    (p) => !Object.keys(currentOverrides).includes(p.name),
  );

  return (
    <Card className="border-border/50 shadow-sm hover:shadow-md transition-all">
      <CardHeader className="pb-3">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-lg bg-teal-500/10 text-teal-600 dark:text-teal-400">
              <LayoutGrid className="w-5 h-5" />
            </div>
            <div className="space-y-1">
              <CardTitle className="text-lg">
                <Trans>Platform Overrides</Trans>
              </CardTitle>
              <CardDescription>
                <Trans>
                  Override configuration (paths, delays, etc.) for specific
                  platforms.
                </Trans>
              </CardDescription>
            </div>
          </div>

          <Popover open={open} onOpenChange={setOpen}>
            <PopoverTrigger asChild>
              <Button variant="outline" size="sm" className="gap-2">
                <Plus className="w-4 h-4" />
                <Trans>Add Override</Trans>
              </Button>
            </PopoverTrigger>
            <PopoverContent className="w-[200px] p-0" align="end">
              <Command>
                <CommandInput placeholder="Search platform..." />
                <CommandList>
                  <CommandEmpty>
                    <Trans>No platform found.</Trans>
                  </CommandEmpty>
                  <CommandGroup>
                    {availablePlatforms.map((platform) => (
                      <CommandItem
                        key={platform.id}
                        value={platform.name}
                        onSelect={() => handleAddOverride(platform.name)}
                      >
                        {platform.name}
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
        {Object.keys(currentOverrides).length === 0 && (
          <div className="flex flex-col items-center justify-center p-8 border-2 border-dashed rounded-lg text-muted-foreground bg-muted/10">
            <LayoutGrid className="w-10 h-10 mb-3 opacity-20" />
            <p className="text-sm font-medium">
              <Trans>No platform overrides configured</Trans>
            </p>
            <p className="text-xs mt-1 max-w-xs text-center">
              <Trans>
                Add an override to customize behavior for specific platforms.
              </Trans>
            </p>
          </div>
        )}
        {Object.keys(currentOverrides).map((platformName) => (
          <PlatformOverrideCard
            key={platformName}
            platformName={platformName}
            form={form}
            onRemove={() => handleRemoveOverride(platformName)}
            engines={engines}
          />
        ))}
      </CardContent>
    </Card>
  );
}
