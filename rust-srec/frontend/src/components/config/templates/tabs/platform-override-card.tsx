import { UseFormReturn } from 'react-hook-form';
import { Button } from '@/components/ui/button';
import {
  Trash2,
  ChevronDown,
  ChevronRight,
  LayoutGrid,
  Settings,
  Boxes,
} from 'lucide-react';
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/components/ui/collapsible';
import { useState } from 'react';
import { Trans } from '@lingui/react/macro';
import { Card, CardHeader, CardContent } from '@/components/ui/card';
import { cn } from '@/lib/utils';
import { EngineConfig } from '@/api/schemas';
import { PlatformSpecificTab } from '../../platforms/tabs/platform-specific-tab';
import { GeneralTab } from '../../platforms/tabs/general-tab';
import {
  SharedConfigEditor,
  SharedConfigPaths,
  ExtraTab,
} from '../../shared-config-editor';

interface PlatformOverrideCardProps {
  platformName: string;
  form: UseFormReturn<any>;
  onRemove: () => void;
  engines?: EngineConfig[];
}

export function PlatformOverrideCard({
  platformName,
  form,
  onRemove,
  engines,
}: PlatformOverrideCardProps) {
  const [isOpen, setIsOpen] = useState(false);

  // Dynamic base path for this override
  const basePath = `platform_overrides.${platformName}`;

  const paths: SharedConfigPaths = {
    streamSelection: `${basePath}.stream_selection_config`,
    cookies: `${basePath}.cookies`,
    proxy: `${basePath}.proxy_config`,
    retryPolicy: `${basePath}.download_retry_policy`,
    output: basePath, // Output settings are flat on the config object (output_folder, etc.)
    limits: basePath, // Limits are flat
    danmu: basePath, // record_danmu is flat
    hooks: `${basePath}.event_hooks`,
    pipeline: `${basePath}.pipeline`,
    sessionCompletePipeline: `${basePath}.session_complete_pipeline`,
    pairedSegmentPipeline: `${basePath}.paired_segment_pipeline`,
  };

  const extraTabs: ExtraTab[] = [
    {
      value: 'general',
      label: <Trans>General</Trans>,
      icon: Settings,
      content: <GeneralTab form={form} basePath={basePath} />,
    },
    {
      value: 'specific',
      label: <Trans>Specific</Trans>,
      icon: Boxes,
      content: (
        <PlatformSpecificTab
          form={form}
          basePath={basePath}
          platformName={platformName}
        />
      ),
    },
  ];

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
            <SharedConfigEditor
              form={form}
              paths={paths}
              engines={engines}
              extraTabs={extraTabs}
              defaultTab="general"
              // Platform override uses objects for complex fields
              proxyMode="object"
              configMode="object"
            />
          </CardContent>
        </CollapsibleContent>
      </Collapsible>
    </Card>
  );
}
