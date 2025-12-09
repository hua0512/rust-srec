import { useState } from 'react';
import { UseFormReturn } from 'react-hook-form';
import { useQuery } from '@tanstack/react-query';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { Plus, Server } from 'lucide-react';
import { engineApi } from '@/api/endpoints';
import { EngineOverrideCard } from './engine-override-card';
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

import { Separator } from '@/components/ui/separator';

interface EngineOverridesTabProps {
    form: UseFormReturn<any>;
}

export function EngineOverridesTab({ form }: EngineOverridesTabProps) {
    const [open, setOpen] = useState(false);

    // Fetch available engines
    const { data: engines = [] } = useQuery({
        queryKey: ['engines'],
        queryFn: engineApi.list,
    });

    const currentOverrides = form.watch('engines_override') || {};
    const overriddenIds = Object.keys(currentOverrides);

    // Filter out engines that are already overridden
    const availableEngines = engines.filter(e => !overriddenIds.includes(e.id));

    const handleAddOverride = (engineId: string) => {
        const engine = engines.find(e => e.id === engineId);
        if (!engine) return;

        // Initialize with existing config or default partial
        // We'll trust the form component to handle undefined nested values, 
        // but it's safer to init with an empty object so the key exists
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
        <div className="space-y-6 animate-in fade-in-50 slide-in-from-left-2 duration-300">
            <div className="flex flex-col gap-4 sm:flex-row sm:items-center sm:justify-between">
                <div className="space-y-1">
                    <h3 className="text-lg font-medium leading-none"><Trans>Engine Overrides</Trans></h3>
                    <p className="text-sm text-muted-foreground">
                        <Trans>Customize specific engine settings for this template.</Trans>
                    </p>
                </div>

                <Popover open={open} onOpenChange={setOpen}>
                    <PopoverTrigger asChild>
                        <Button variant="outline" className="w-[200px] justify-between">
                            <><Plus className="mr-2 h-4 w-4" /><Trans>Add Override</Trans></>
                            <Server className="ml-2 h-4 w-4 opacity-50" />
                        </Button>
                    </PopoverTrigger>
                    <PopoverContent className="w-[300px] p-0" align="end">
                        <Command>
                            <CommandInput placeholder={t`Search engines...`} />
                            <CommandList>
                                <CommandEmpty><Trans>No engines found.</Trans></CommandEmpty>
                                <CommandGroup>
                                    {availableEngines.map((engine) => (
                                        <CommandItem
                                            key={engine.id}
                                            value={engine.name}
                                            onSelect={() => handleAddOverride(engine.id)}
                                        >
                                            <div className="flex flex-col">
                                                <span>{engine.name}</span>
                                                <span className="text-xs text-muted-foreground">{engine.engine_type}</span>
                                            </div>
                                        </CommandItem>
                                    ))}
                                </CommandGroup>
                            </CommandList>
                        </Command>
                    </PopoverContent>
                </Popover>
            </div>

            <Separator />

            <div className="grid gap-6">
                {overriddenIds.length === 0 ? (
                    <div className="flex flex-col items-center justify-center p-8 border-2 border-dashed rounded-lg text-muted-foreground bg-muted/10">
                        <Server className="w-10 h-10 mb-3 opacity-20" />
                        <p className="text-sm font-medium"><Trans>No overrides configured</Trans></p>
                        <p className="text-xs mt-1 max-w-xs text-center"><Trans>Add an override to customize engine behavior for this template.</Trans></p>
                    </div>
                ) : (
                    overriddenIds.map((engineId) => {
                        const engine = engines.find(e => e.id === engineId);
                        // Even if engine is deleted from system, if we have an override for it, we should probably show something or allow deletion
                        if (!engine) {
                            return (
                                <div key={engineId} className="flex items-center justify-between p-4 border rounded bg-destructive/10 text-destructive">
                                    <span><Trans>Unknown Engine ID: {engineId}</Trans></span>
                                    <Button variant="ghost" size="sm" onClick={() => handleRemoveOverride(engineId)}>
                                        <Trans>Remove</Trans>
                                    </Button>
                                </div>
                            );
                        }

                        return (
                            <EngineOverrideCard
                                key={engineId}
                                engineId={engineId}
                                engineName={engine.name}
                                engineType={engine.engine_type}
                                form={form}
                                onRemove={() => handleRemoveOverride(engineId)}
                            />
                        );
                    })
                )}
            </div>
        </div>
    );
}
