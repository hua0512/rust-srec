
import { UseFormReturn } from "react-hook-form";
import { Button } from "../../../ui/button";
import { Trash2, ChevronDown, ChevronRight, LayoutGrid } from "lucide-react";
import {
    Collapsible,
    CollapsibleContent,
    CollapsibleTrigger,
} from "../../../ui/collapsible";
import { useState } from "react";
import { Trans } from "@lingui/react/macro";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "../../../ui/tabs";
import { GeneralTab } from "../../platforms/tabs/general-tab";
import { AuthTab } from "../../platforms/tabs/auth-tab";
import { ProxyTab } from "../../platforms/tabs/proxy-tab";
import { StreamSelectionTab } from "../../platforms/tabs/stream-selection-tab";
import { AdvancedTab } from "../../platforms/tabs/advanced-tab";
import { Card } from "../../../ui/card";

interface PlatformOverrideCardProps {
    platformName: string;
    form: UseFormReturn<any>;
    onRemove: () => void;
}

export function PlatformOverrideCard({ platformName, form, onRemove }: PlatformOverrideCardProps) {
    const [isOpen, setIsOpen] = useState(false);

    // Dynamic base path for this override
    // Stores inside "platform_overrides" map, keyed by platformName
    const basePath = `platform_overrides.${platformName}`;

    return (
        <Collapsible open={isOpen} onOpenChange={setIsOpen} className="space-y-2">
            <div className="flex items-center justify-between p-4 rounded-xl border bg-card/50 hover:bg-card hover:shadow-sm transition-all shadow-sm">
                <div className="flex items-center gap-4">
                    <CollapsibleTrigger asChild>
                        <Button variant="ghost" size="sm" className="w-8 h-8 p-0 hover:bg-muted/80">
                            {isOpen ? (
                                <ChevronDown className="h-4 w-4" />
                            ) : (
                                <ChevronRight className="h-4 w-4" />
                            )}
                            <span className="sr-only">Toggle</span>
                        </Button>
                    </CollapsibleTrigger>

                    <div className="flex items-center gap-3">
                        <div className="p-2 bg-primary/10 rounded-lg">
                            <LayoutGrid className="w-4 h-4 text-primary" />
                        </div>
                        <div className="flex flex-col">
                            <h4 className="font-medium text-sm">{platformName}</h4>
                            <span className="text-xs text-muted-foreground mr-2">
                                <Trans>Override Settings</Trans>
                            </span>
                        </div>
                    </div>
                </div>

                <div className="flex items-center gap-2">
                    <Button
                        variant="ghost"
                        size="icon"
                        className="h-8 w-8 text-destructive hover:text-destructive hover:bg-destructive/10"
                        onClick={onRemove}
                    >
                        <Trash2 className="h-4 w-4" />
                    </Button>
                </div>
            </div>

            <CollapsibleContent className="space-y-4 animate-in slide-in-from-top-2 duration-200">
                <Card className="p-4 border-dashed bg-muted/10 mx-2">
                    <Tabs defaultValue="general" className="w-full">
                        <TabsList className="flex flex-wrap h-auto w-full justify-start gap-2 bg-transparent p-0 mb-4 border-b pb-2">
                            <TabsTrigger value="general" className="gap-2 h-8 text-xs px-3 data-[state=active]:bg-primary/10 data-[state=active]:text-primary border bg-background hover:bg-accent hover:text-accent-foreground transition-all shadow-sm rounded-md">
                                <Trans>General</Trans>
                            </TabsTrigger>
                            <TabsTrigger value="auth" className="gap-2 h-8 text-xs px-3 data-[state=active]:bg-orange-500/10 data-[state=active]:text-orange-600 border bg-background hover:bg-accent hover:text-accent-foreground transition-all shadow-sm rounded-md">
                                <Trans>Auth</Trans>
                            </TabsTrigger>
                            <TabsTrigger value="stream-selection" className="gap-2 h-8 text-xs px-3 data-[state=active]:bg-blue-500/10 data-[state=active]:text-blue-600 border bg-background hover:bg-accent hover:text-accent-foreground transition-all shadow-sm rounded-md">
                                <Trans>Streams</Trans>
                            </TabsTrigger>
                            <TabsTrigger value="proxy" className="gap-2 h-8 text-xs px-3 data-[state=active]:bg-green-500/10 data-[state=active]:text-green-600 border bg-background hover:bg-accent hover:text-accent-foreground transition-all shadow-sm rounded-md">
                                <Trans>Proxy</Trans>
                            </TabsTrigger>
                            <TabsTrigger value="advanced" className="gap-2 h-8 text-xs px-3 data-[state=active]:bg-purple-500/10 data-[state=active]:text-purple-600 border bg-background hover:bg-accent hover:text-accent-foreground transition-all shadow-sm rounded-md">
                                <Trans>Advanced</Trans>
                            </TabsTrigger>
                        </TabsList>

                        <div className="mt-2 pl-1 pr-1">
                            <TabsContent value="general" className="mt-0">
                                <GeneralTab form={form} basePath={basePath} />
                            </TabsContent>
                            <TabsContent value="auth" className="mt-0">
                                <AuthTab form={form} basePath={basePath} />
                            </TabsContent>
                            <TabsContent value="stream-selection" className="mt-0">
                                <StreamSelectionTab form={form} basePath={basePath} />
                            </TabsContent>
                            <TabsContent value="proxy" className="mt-0">
                                <ProxyTab form={form} basePath={basePath} />
                            </TabsContent>
                            <TabsContent value="advanced" className="mt-0">
                                <AdvancedTab form={form} basePath={basePath} />
                            </TabsContent>
                        </div>
                    </Tabs>
                </Card>
            </CollapsibleContent>
        </Collapsible>
    );
}
