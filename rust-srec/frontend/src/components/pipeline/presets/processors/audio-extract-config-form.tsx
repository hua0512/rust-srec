import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import {
  FormField,
  FormItem,
  FormLabel,
  FormControl,
  FormMessage,
  FormDescription,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Switch } from '@/components/ui/switch';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { ProcessorConfigFormProps } from './common-props';
import { AudioExtractConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { motion } from 'motion/react';
import { Music, AudioWaveform, Settings2 } from 'lucide-react';

type AudioExtractConfig = z.infer<typeof AudioExtractConfigSchema>;

export function AudioExtractConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps<AudioExtractConfig>) {
  const { i18n } = useLingui();
  const prefix = pathPrefix ? `${pathPrefix}.` : '';

  const containerVariants = {
    hidden: { opacity: 0, y: 20 },
    visible: { opacity: 1, y: 0, transition: { duration: 0.3 } },
  };

  return (
    <motion.div
      variants={containerVariants}
      initial="hidden"
      animate="visible"
      className="w-full"
    >
      <Tabs defaultValue="general" className="w-full">
        <TabsList className="w-full h-auto p-1.5 bg-muted/20 rounded-xl grid grid-cols-2 gap-1.5 backdrop-blur-sm mb-6">
          <TabsTrigger
            value="general"
            className="flex flex-col sm:flex-row items-center justify-center gap-1.5 sm:gap-2 h-auto py-2.5 rounded-lg data-[state=active]:bg-background data-[state=active]:text-primary data-[state=active]:shadow-sm transition-all duration-300 relative overflow-hidden group"
          >
            <Settings2 className="w-4 h-4 shrink-0 transition-transform duration-300 group-data-[state=active]:scale-110" />
            <span className="text-xs font-medium hidden sm:inline">
              <Trans>General</Trans>
            </span>
          </TabsTrigger>
          <TabsTrigger
            value="advanced"
            className="flex flex-col sm:flex-row items-center justify-center gap-1.5 sm:gap-2 h-auto py-2.5 rounded-lg data-[state=active]:bg-background data-[state=active]:text-primary data-[state=active]:shadow-sm transition-all duration-300 relative overflow-hidden group"
          >
            <AudioWaveform className="w-4 h-4 shrink-0 transition-transform duration-300 group-data-[state=active]:scale-110" />
            <span className="text-xs font-medium hidden sm:inline">
              <Trans>Advanced</Trans>
            </span>
          </TabsTrigger>
        </TabsList>

        <div className="space-y-4">
          <TabsContent
            value="general"
            className="space-y-4 focus-visible:outline-none focus-visible:ring-0"
          >
            <div className="space-y-4 p-4 rounded-xl bg-muted/10 border border-border/40">
              <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
                <Music className="w-4 h-4 text-pink-500" />
                <h3 className="font-semibold text-sm mr-auto">
                  <Trans>Output Settings</Trans>
                </h3>
              </div>

              <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                <FormField
                  control={control}
                  name={`${prefix}format` as any}
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel className="text-xs text-muted-foreground ml-1">
                        <Trans>Format</Trans>
                      </FormLabel>
                      <Select
                        onValueChange={field.onChange}
                        defaultValue={field.value || undefined}
                      >
                        <FormControl>
                          <SelectTrigger className="h-11 bg-background/50 border-border/50 focus:bg-background transition-colors rounded-lg">
                            <SelectValue
                              placeholder={i18n._(msg`Copy (Stream Copy)`)}
                            />
                          </SelectTrigger>
                        </FormControl>
                        <SelectContent>
                          <SelectItem value="mp3">MP3</SelectItem>
                          <SelectItem value="aac">AAC</SelectItem>
                          <SelectItem value="flac">FLAC</SelectItem>
                          <SelectItem value="opus">Opus</SelectItem>
                        </SelectContent>
                      </Select>
                      <FormDescription className="text-[11px] ml-1">
                        <Trans>Leave empty for stream copy</Trans>
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />

                <FormField
                  control={control}
                  name={`${prefix}bitrate` as any}
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel className="text-xs text-muted-foreground ml-1">
                        <Trans>Bitrate</Trans>
                      </FormLabel>
                      <FormControl>
                        <Input
                          className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                          placeholder={i18n._(msg`128k`)}
                          {...field}
                        />
                      </FormControl>
                      <FormDescription className="text-[11px] ml-1">
                        <Trans>e.g. 128k, 320k</Trans>
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </div>
            </div>
          </TabsContent>

          <TabsContent
            value="advanced"
            className="space-y-6 focus-visible:outline-none focus-visible:ring-0"
          >
            <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
              <div className="flex items-center gap-2 pb-2 border-b border-border/40">
                <AudioWaveform className="w-4 h-4 text-indigo-500" />
                <h3 className="font-semibold text-sm mr-auto">
                  <Trans>Audio Properties</Trans>
                </h3>
              </div>

              <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                <FormField
                  control={control}
                  name={`${prefix}sample_rate` as any}
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel className="text-xs text-muted-foreground ml-1">
                        <Trans>Sample Rate (Hz)</Trans>
                      </FormLabel>
                      <FormControl>
                        <Input
                          className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                          type="number"
                          placeholder={i18n._(msg`44100`)}
                          {...field}
                          onChange={(e) =>
                            field.onChange(
                              e.target.value
                                ? parseInt(e.target.value)
                                : undefined,
                            )
                          }
                        />
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />

                <FormField
                  control={control}
                  name={`${prefix}channels` as any}
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel className="text-xs text-muted-foreground ml-1">
                        <Trans>Channels</Trans>
                      </FormLabel>
                      <FormControl>
                        <Input
                          className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                          type="number"
                          min={1}
                          max={8}
                          placeholder={i18n._(msg`2`)}
                          {...field}
                          onChange={(e) =>
                            field.onChange(
                              e.target.value
                                ? parseInt(e.target.value)
                                : undefined,
                            )
                          }
                        />
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </div>
            </div>

            <div className="space-y-3">
              <FormField
                control={control}
                name={`${prefix}overwrite` as any}
                render={({ field }) => (
                  <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 p-4 shadow-sm bg-muted/10 transition-colors hover:bg-muted/20">
                    <div className="space-y-1">
                      <FormLabel className="text-sm font-medium">
                        <Trans>Overwrite Files</Trans>
                      </FormLabel>
                      <FormDescription className="text-xs">
                        <Trans>
                          Replace output files if they already exist
                        </Trans>
                      </FormDescription>
                    </div>
                    <FormControl>
                      <Switch
                        checked={field.value}
                        onCheckedChange={field.onChange}
                      />
                    </FormControl>
                  </FormItem>
                )}
              />
            </div>
          </TabsContent>
        </div>
      </Tabs>
    </motion.div>
  );
}
