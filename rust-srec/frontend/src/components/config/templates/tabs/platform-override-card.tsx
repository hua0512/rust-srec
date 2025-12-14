import { UseFormReturn } from 'react-hook-form';
import { Button } from '../../../ui/button';
import {
  Trash2,
  ChevronDown,
  ChevronRight,
  LayoutGrid,
  Settings,
  Cookie,
  Filter,
  Shield,
  Code,
} from 'lucide-react';
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '../../../ui/collapsible';
import { useState } from 'react';
import { Trans } from '@lingui/react/macro';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../../../ui/tabs';
import { GeneralTab } from '../../platforms/tabs/general-tab';
import { AuthTab } from '../../platforms/tabs/auth-tab';
import { ProxyTab } from '../../platforms/tabs/proxy-tab';
import { StreamSelectionTab } from '../../platforms/tabs/stream-selection-tab';
import { AdvancedTab } from '../../platforms/tabs/advanced-tab';
import { Card, CardHeader, CardContent } from '../../../ui/card';
import { cn } from '@/lib/utils';

interface PlatformOverrideCardProps {
  platformName: string;
  form: UseFormReturn<any>;
  onRemove: () => void;
}

export function PlatformOverrideCard({
  platformName,
  form,
  onRemove,
}: PlatformOverrideCardProps) {
  const [isOpen, setIsOpen] = useState(false);

  // Dynamic base path for this override
  const basePath = `platform_overrides.${platformName}`;

  return (
    <Card
      className={cn(
        'border-border/50 transition-all duration-200',
        isOpen
          ? 'shadow-md ring-1 ring-primary/5 border-primary/20'
          : 'shadow-sm hover:shadow-md hover:border-primary/20',
      )}
    >
      <Collapsible open={isOpen} onOpenChange={setIsOpen}>
        <CardHeader className="p-0">
          <div className="flex items-center justify-between p-4">
            <div
              className="flex items-center gap-4 flex-1 cursor-pointer"
              onClick={() => setIsOpen(!isOpen)}
            >
              <CollapsibleTrigger asChild>
                <Button
                  variant="ghost"
                  size="sm"
                  className="w-8 h-8 p-0 hover:bg-muted/80 shrink-0"
                >
                  {isOpen ? (
                    <ChevronDown className="h-4 w-4" />
                  ) : (
                    <ChevronRight className="h-4 w-4" />
                  )}
                  <span className="sr-only">Toggle</span>
                </Button>
              </CollapsibleTrigger>

              <div className="flex items-center gap-3">
                <div className="p-2 bg-teal-500/10 text-teal-600 dark:text-teal-400 rounded-lg">
                  <LayoutGrid className="w-5 h-5" />
                </div>
                <div className="flex flex-col">
                  <h4 className="font-semibold text-base">{platformName}</h4>
                  <span className="text-xs text-muted-foreground">
                    <Trans>Platform Override Settings</Trans>
                  </span>
                </div>
              </div>
            </div>

            <Button
              variant="ghost"
              size="icon"
              className="h-9 w-9 text-muted-foreground hover:text-destructive hover:bg-destructive/10 transition-colors"
              onClick={(e) => {
                e.stopPropagation();
                onRemove();
              }}
            >
              <Trash2 className="h-4 w-4" />
            </Button>
          </div>
        </CardHeader>

        <CollapsibleContent className="animate-in slide-in-from-top-2 fade-in duration-200">
          <CardContent className="pt-0 pb-6 px-4 sm:px-6 border-t border-border/40 bg-muted/5 mt-4">
            <Tabs defaultValue="general" className="w-full pt-4">
              <TabsList className="h-auto bg-background/50 p-1 gap-1 flex-wrap justify-start w-full border mb-6 rounded-lg">
                <TabsTrigger
                  value="general"
                  className="gap-2 px-3 py-1.5 h-8 text-xs data-[state=active]:bg-primary/10 data-[state=active]:text-primary rounded-md transition-all"
                >
                  <Settings className="w-3.5 h-3.5" />
                  <Trans>General</Trans>
                </TabsTrigger>
                <TabsTrigger
                  value="auth"
                  className="gap-2 px-3 py-1.5 h-8 text-xs data-[state=active]:bg-orange-500/10 data-[state=active]:text-orange-600 rounded-md transition-all"
                >
                  <Cookie className="w-3.5 h-3.5" />
                  <Trans>Auth</Trans>
                </TabsTrigger>
                <TabsTrigger
                  value="stream-selection"
                  className="gap-2 px-3 py-1.5 h-8 text-xs data-[state=active]:bg-blue-500/10 data-[state=active]:text-blue-600 rounded-md transition-all"
                >
                  <Filter className="w-3.5 h-3.5" />
                  <Trans>Streams</Trans>
                </TabsTrigger>
                <TabsTrigger
                  value="proxy"
                  className="gap-2 px-3 py-1.5 h-8 text-xs data-[state=active]:bg-green-500/10 data-[state=active]:text-green-600 rounded-md transition-all"
                >
                  <Shield className="w-3.5 h-3.5" />
                  <Trans>Proxy</Trans>
                </TabsTrigger>
                <TabsTrigger
                  value="advanced"
                  className="gap-2 px-3 py-1.5 h-8 text-xs data-[state=active]:bg-purple-500/10 data-[state=active]:text-purple-600 rounded-md transition-all"
                >
                  <Code className="w-3.5 h-3.5" />
                  <Trans>Advanced</Trans>
                </TabsTrigger>
              </TabsList>

              <div className="mt-2">
                <TabsContent
                  value="general"
                  className="mt-0 focus-visible:outline-none animate-in fade-in-50 slide-in-from-bottom-1 duration-300"
                >
                  <GeneralTab form={form} basePath={basePath} />
                </TabsContent>
                <TabsContent
                  value="auth"
                  className="mt-0 focus-visible:outline-none animate-in fade-in-50 slide-in-from-bottom-1 duration-300"
                >
                  <AuthTab form={form} basePath={basePath} />
                </TabsContent>
                <TabsContent
                  value="stream-selection"
                  className="mt-0 focus-visible:outline-none animate-in fade-in-50 slide-in-from-bottom-1 duration-300"
                >
                  <StreamSelectionTab form={form} basePath={basePath} />
                </TabsContent>
                <TabsContent
                  value="proxy"
                  className="mt-0 focus-visible:outline-none animate-in fade-in-50 slide-in-from-bottom-1 duration-300"
                >
                  <ProxyTab form={form} basePath={basePath} />
                </TabsContent>
                <TabsContent
                  value="advanced"
                  className="mt-0 focus-visible:outline-none animate-in fade-in-50 slide-in-from-bottom-1 duration-300"
                >
                  <AdvancedTab form={form} basePath={basePath} />
                </TabsContent>
              </div>
            </Tabs>
          </CardContent>
        </CollapsibleContent>
      </Collapsible>
    </Card>
  );
}
