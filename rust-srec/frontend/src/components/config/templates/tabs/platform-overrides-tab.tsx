
import { UseFormReturn } from "react-hook-form";
import { useQuery } from "@tanstack/react-query";
import { listPlatformConfigs } from "@/server/functions";
import { Button } from "../../../ui/button";
import { Plus, Settings } from "lucide-react";
import {
    Popover,
    PopoverContent,
    PopoverTrigger,
} from "../../../ui/popover";
import {
    Command,
    CommandEmpty,
    CommandGroup,
    CommandInput,
    CommandItem,
    CommandList,
} from "../../../ui/command";
import { useState } from "react";
import { Trans } from "@lingui/react/macro";

import { PlatformOverrideCard } from "./platform-override-card";


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
    const availablePlatforms = platforms.filter(p => !Object.keys(currentOverrides).includes(p.name));

    return (
        <div className="space-y-4">
            <div className="flex flex-col sm:flex-row items-start sm:items-center justify-between gap-4 rounded-lg bg-muted/30 p-4 border border-border/50">
                <div className="space-y-1">
                    <h3 className="text-base font-semibold flex items-center gap-2">
                        <Settings className="w-4 h-4 text-primary" />
                        <Trans>Platform Specific Overrides</Trans>
                    </h3>
                    <p className="text-sm text-muted-foreground">
                        <Trans>Override configuration (paths, delays, etc.) for specific platforms within this template.</Trans>
                    </p>
                </div>

                <Popover open={open} onOpenChange={setOpen}>
                    <PopoverTrigger asChild>
                        <Button variant="outline" className="gap-2 shadow-sm border-dashed">
                            <Plus className="w-4 h-4" />
                            <Trans>Add Override</Trans>
                        </Button>
                    </PopoverTrigger>
                    <PopoverContent className="w-[200px] p-0" align="end">
                        <Command>
                            <CommandInput placeholder="Search platform..." />
                            <CommandList>
                                <CommandEmpty><Trans>No platform found.</Trans></CommandEmpty>
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

            <div className="space-y-4">
                {Object.keys(currentOverrides).length === 0 && (
                    <div className="text-center py-12 border-2 border-dashed rounded-xl bg-muted/20">
                        <p className="text-muted-foreground text-sm"><Trans>No platform overrides configured.</Trans></p>
                    </div>
                )}
                {Object.keys(currentOverrides).map((platformName) => (
                    <PlatformOverrideCard
                        key={platformName}
                        platformName={platformName}
                        form={form}
                        onRemove={() => handleRemoveOverride(platformName)}
                    />
                ))}
            </div>
        </div>
    );
}
