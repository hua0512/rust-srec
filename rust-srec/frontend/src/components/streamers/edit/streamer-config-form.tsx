import { UseFormReturn } from 'react-hook-form';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import { Trans } from '@lingui/react/macro';
import { motion } from 'motion/react';
import { StreamerConfiguration } from '@/components/streamers/config/streamer-configuration';
import { StreamerGeneralSettings } from '@/components/streamers/config/streamer-general-settings';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { Settings, Activity } from 'lucide-react';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { CheckCircle2 } from 'lucide-react';

import { PlatformConfig, Template, EngineConfig } from '@/api/schemas';

interface StreamerConfigFormProps {
  form: UseFormReturn<any>;
  availablePlatforms: PlatformConfig[];
  templates: Template[] | undefined;
  detectedPlatform?: string | null;
  engines?: EngineConfig[];
}

const tabContentVariants = {
  hidden: { opacity: 0, x: -10 },
  visible: { opacity: 1, x: 0, transition: { duration: 0.2 } },
};

export function StreamerConfigForm({
  form,
  availablePlatforms,
  templates,
  detectedPlatform,
  engines,
}: StreamerConfigFormProps) {
  return (
    <div className="space-y-6">
      {detectedPlatform && (
        <motion.div
          initial={{ opacity: 0, y: -20 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.3 }}
        >
          <Alert className="bg-primary/5 border-primary/20">
            <CheckCircle2 className="h-4 w-4 text-primary" />
            <AlertTitle className="text-primary font-medium">
              <Trans>Platform Detected: {detectedPlatform}</Trans>
            </AlertTitle>
            <AlertDescription className="text-muted-foreground text-xs">
              <Trans>Settings have been optimized for this platform.</Trans>
            </AlertDescription>
          </Alert>
        </motion.div>
      )}

      <Tabs defaultValue="general" className="w-full">
        <TabsList className="grid w-full grid-cols-2 h-auto p-1 bg-muted/30 border rounded-xl md:rounded-full md:inline-flex md:w-auto backdrop-blur-sm">
          <TabsTrigger
            value="general"
            className="rounded-lg md:rounded-full px-6 py-2.5 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm transition-all"
          >
            <Settings className="w-4 h-4 mr-2" />
            <Trans>General</Trans>
          </TabsTrigger>
          <TabsTrigger
            value="advanced"
            className="rounded-lg md:rounded-full px-6 py-2.5 data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm transition-all"
          >
            <Activity className="w-4 h-4 mr-2" />
            <Trans>Advanced</Trans>
          </TabsTrigger>
        </TabsList>

        <TabsContent
          value="general"
          className="mt-6 border-none ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
        >
          <motion.div
            variants={tabContentVariants}
            initial="hidden"
            animate="visible"
            key="general"
          >
            <Card className="border-border/40 shadow-sm bg-card/80 backdrop-blur-sm">
              <CardHeader>
                <CardTitle>
                  <Trans>General Configuration</Trans>
                </CardTitle>
                <CardDescription>
                  <Trans>Basic settings for the streamer.</Trans>
                </CardDescription>
              </CardHeader>
              <CardContent>
                <StreamerGeneralSettings
                  form={form}
                  platformConfigs={availablePlatforms}
                  templates={templates}
                />
              </CardContent>
            </Card>
          </motion.div>
        </TabsContent>

        <TabsContent
          value="advanced"
          className="mt-6 border-none ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
        >
          <motion.div
            variants={tabContentVariants}
            initial="hidden"
            animate="visible"
            key="advanced"
          >
            <Card className="border-border/40 shadow-sm bg-card/80 backdrop-blur-sm">
              <CardHeader>
                <CardTitle>
                  <Trans>Advanced Configuration</Trans>
                </CardTitle>
                <CardDescription>
                  <Trans>Override global defaults for this streamer.</Trans>
                </CardDescription>
              </CardHeader>
              <CardContent>
                <StreamerConfiguration form={form} engines={engines} />
              </CardContent>
            </Card>
          </motion.div>
        </TabsContent>
      </Tabs>
    </div>
  );
}
