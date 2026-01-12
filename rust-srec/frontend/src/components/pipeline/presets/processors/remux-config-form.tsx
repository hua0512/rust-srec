import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { cn } from '@/lib/utils';
import { memo, useEffect } from 'react';
import { useWatch, useFormContext } from 'react-hook-form';
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
import { ListInput } from '@/components/ui/list-input';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import { ProcessorConfigFormProps } from './common-props';
import { RemuxConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { InputWithUnit } from '@/components/ui/input-with-unit';
import { motion } from 'motion/react';
import {
  Settings2,
  Film,
  Music,
  Sliders,
  Box,
  Layers,
  Play,
  Square,
  Timer,
  MousePointer2,
  Plus,
  Trash2,
  Tags,
} from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Slider } from '@/components/ui/slider';

type RemuxConfig = z.infer<typeof RemuxConfigSchema>;

export const RemuxConfigForm = memo(function RemuxConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps<RemuxConfig>) {
  const { i18n } = useLingui();
  const prefix = pathPrefix ? `${pathPrefix}.` : '';
  const { setValue } = useFormContext();

  const videoCodec = useWatch({
    control,
    name: `${prefix}video_codec` as any,
  });

  const audioCodec = useWatch({
    control,
    name: `${prefix}audio_codec` as any,
  });

  const format = useWatch({
    control,
    name: `${prefix}format` as any,
  });

  const isVideoCopy = !videoCodec || videoCodec === 'copy';
  const isAudioCopy = !audioCodec || audioCodec === 'copy';

  const faststartSupported =
    !format || ['mp4', 'mov', 'm4v'].includes(format.toLowerCase());

  useEffect(() => {
    if (!faststartSupported) {
      setValue(`${prefix}faststart` as any, false);
    }
  }, [faststartSupported, prefix, setValue]);

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
        <TabsList className="w-full h-auto p-1.5 bg-muted/20 rounded-xl flex flex-wrap gap-1.5 backdrop-blur-sm mb-6">
          <TabsTrigger
            value="general"
            className="flex-1 min-w-[100px] flex flex-col sm:flex-row items-center justify-center gap-1.5 sm:gap-2 h-auto py-2.5 rounded-lg data-[state=active]:bg-background data-[state=active]:text-primary data-[state=active]:shadow-sm transition-all duration-300 relative overflow-hidden group"
          >
            <Settings2 className="w-4 h-4 shrink-0 transition-transform duration-300 group-data-[state=active]:scale-110" />
            <span className="text-xs font-medium hidden sm:inline">
              <Trans>General</Trans>
            </span>
          </TabsTrigger>
          <TabsTrigger
            value="format"
            className="flex-1 min-w-[100px] flex flex-col sm:flex-row items-center justify-center gap-1.5 sm:gap-2 h-auto py-2.5 rounded-lg data-[state=active]:bg-background data-[state=active]:text-primary data-[state=active]:shadow-sm transition-all duration-300 relative overflow-hidden group"
          >
            <Box className="w-4 h-4 shrink-0 transition-transform duration-300 group-data-[state=active]:scale-110" />
            <span className="text-xs font-medium hidden sm:inline">
              <Trans>Format</Trans>
            </span>
          </TabsTrigger>
          <TabsTrigger
            value="video"
            className="flex-1 min-w-[100px] flex flex-col sm:flex-row items-center justify-center gap-1.5 sm:gap-2 h-auto py-2.5 rounded-lg data-[state=active]:bg-background data-[state=active]:text-primary data-[state=active]:shadow-sm transition-all duration-300 relative overflow-hidden group"
          >
            <Film className="w-4 h-4 shrink-0 transition-transform duration-300 group-data-[state=active]:scale-110" />
            <span className="text-xs font-medium hidden sm:inline">
              <Trans>Video</Trans>
            </span>
          </TabsTrigger>
          <TabsTrigger
            value="audio"
            className="flex-1 min-w-[100px] flex flex-col sm:flex-row items-center justify-center gap-1.5 sm:gap-2 h-auto py-2.5 rounded-lg data-[state=active]:bg-background data-[state=active]:text-primary data-[state=active]:shadow-sm transition-all duration-300 relative overflow-hidden group"
          >
            <Music className="w-4 h-4 shrink-0 transition-transform duration-300 group-data-[state=active]:scale-110" />
            <span className="text-xs font-medium hidden sm:inline">
              <Trans>Audio</Trans>
            </span>
          </TabsTrigger>
          <TabsTrigger
            value="advanced"
            className="flex-1 min-w-[100px] flex flex-col sm:flex-row items-center justify-center gap-1.5 sm:gap-2 h-auto py-2.5 rounded-lg data-[state=active]:bg-background data-[state=active]:text-primary data-[state=active]:shadow-sm transition-all duration-300 relative overflow-hidden group"
          >
            <Sliders className="w-4 h-4 shrink-0 transition-transform duration-300 group-data-[state=active]:scale-110" />
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
            <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
              {/* Video Section */}
              <div className="space-y-4 p-4 rounded-xl bg-muted/10 border border-border/40">
                <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
                  <Film className="w-4 h-4 text-purple-500" />
                  <h3 className="font-semibold text-sm mr-auto">
                    <Trans>Video Settings</Trans>
                  </h3>
                </div>

                <FormField
                  control={control}
                  name={`${prefix}video_codec` as any}
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel className="text-xs text-muted-foreground ml-1">
                        <Trans>Video Codec</Trans>
                      </FormLabel>
                      <Select
                        onValueChange={field.onChange}
                        value={field.value || 'copy'}
                      >
                        <FormControl>
                          <SelectTrigger className="h-11 bg-background/50 border-border/50 focus:bg-background transition-colors rounded-lg">
                            <SelectValue placeholder="Select codec" />
                          </SelectTrigger>
                        </FormControl>
                        <SelectContent>
                          <SelectItem value="copy">
                            <Trans>Copy (Passthrough)</Trans>
                          </SelectItem>
                          <SelectItem value="h264">H.264</SelectItem>
                          <SelectItem value="h265">H.265 / HEVC</SelectItem>
                          <SelectItem value="vp9">VP9</SelectItem>
                          <SelectItem value="av1">AV1</SelectItem>
                        </SelectContent>
                      </Select>
                      <FormMessage />
                    </FormItem>
                  )}
                />

                {!isVideoCopy && (
                  <FormField
                    control={control}
                    name={`${prefix}preset` as any}
                    render={({ field }) => (
                      <FormItem>
                        <div className="flex justify-between items-center mb-1">
                          <FormLabel className="text-xs text-muted-foreground ml-1">
                            <Trans>Encoding Preset</Trans>
                          </FormLabel>
                          <span className="text-[10px] text-muted-foreground/70 uppercase tracking-wider">
                            {field.value || 'medium'}
                          </span>
                        </div>
                        <Select
                          onValueChange={field.onChange}
                          value={field.value || 'medium'}
                        >
                          <FormControl>
                            <SelectTrigger className="h-11 bg-background/50 border-border/50 focus:bg-background transition-colors rounded-lg">
                              <SelectValue placeholder="Select preset" />
                            </SelectTrigger>
                          </FormControl>
                          <SelectContent>
                            <SelectItem value="ultrafast">Ultrafast</SelectItem>
                            <SelectItem value="superfast">Superfast</SelectItem>
                            <SelectItem value="veryfast">Veryfast</SelectItem>
                            <SelectItem value="faster">Faster</SelectItem>
                            <SelectItem value="fast">Fast</SelectItem>
                            <SelectItem value="medium">Medium</SelectItem>
                            <SelectItem value="slow">Slow</SelectItem>
                            <SelectItem value="slower">Slower</SelectItem>
                            <SelectItem value="veryslow">Veryslow</SelectItem>
                          </SelectContent>
                        </Select>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                )}
              </div>

              {/* Audio & Container Section */}
              <div className="space-y-4">
                <div className="space-y-4 p-4 rounded-xl bg-muted/10 border border-border/40">
                  <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
                    <Music className="w-4 h-4 text-pink-500" />
                    <h3 className="font-semibold text-sm mr-auto">
                      <Trans>Audio Settings</Trans>
                    </h3>
                  </div>
                  <FormField
                    control={control}
                    name={`${prefix}audio_codec` as any}
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel className="text-xs text-muted-foreground ml-1">
                          <Trans>Audio Codec</Trans>
                        </FormLabel>
                        <Select
                          onValueChange={field.onChange}
                          value={field.value || 'copy'}
                        >
                          <FormControl>
                            <SelectTrigger className="h-11 bg-background/50 border-border/50 focus:bg-background transition-colors rounded-lg">
                              <SelectValue placeholder="Select codec" />
                            </SelectTrigger>
                          </FormControl>
                          <SelectContent>
                            <SelectItem value="copy">
                              <Trans>Copy (Passthrough)</Trans>
                            </SelectItem>
                            <SelectItem value="aac">AAC</SelectItem>
                            <SelectItem value="mp3">MP3</SelectItem>
                            <SelectItem value="opus">Opus</SelectItem>
                            <SelectItem value="flac">FLAC</SelectItem>
                            <SelectItem value="none">
                              <Trans>None (Strip Audio)</Trans>
                            </SelectItem>
                          </SelectContent>
                        </Select>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                </div>

                <div className="space-y-4 p-4 rounded-xl bg-muted/10 border border-border/40">
                  <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
                    <Box className="w-4 h-4 text-blue-500" />
                    <h3 className="font-semibold text-sm mr-auto">
                      <Trans>Container</Trans>
                    </h3>
                  </div>
                  <FormField
                    control={control}
                    name={`${prefix}format` as any}
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel className="text-xs text-muted-foreground ml-1">
                          <Trans>Output Format</Trans>
                        </FormLabel>
                        <FormControl>
                          <Input
                            className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg"
                            placeholder="mp4"
                            {...field}
                            value={field.value ?? ''}
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                </div>
              </div>
            </div>
          </TabsContent>

          <TabsContent
            value="format"
            className="space-y-6 focus-visible:outline-none focus-visible:ring-0"
          >
            {/* Quality Section */}
            {!isVideoCopy && (
              <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
                <div className="flex items-center gap-2 pb-2 border-b border-border/40">
                  <Layers className="w-4 h-4 text-emerald-500" />
                  <h3 className="font-semibold text-sm mr-auto">
                    <Trans>Quality Control</Trans>
                  </h3>
                </div>

                <FormField
                  control={control}
                  name={`${prefix}crf` as any}
                  render={({ field }) => (
                    <FormItem>
                      <div className="flex justify-between items-end mb-2">
                        <FormLabel className="text-xs text-muted-foreground ml-1">
                          <Trans>CRF (Constant Rate Factor)</Trans>
                        </FormLabel>
                        <span className="font-mono text-sm font-medium bg-background px-2 py-0.5 rounded border border-border/50">
                          {field.value ?? 23}
                        </span>
                      </div>
                      <div className="flex items-center gap-4">
                        <span className="text-[10px] text-muted-foreground font-medium w-8 text-right">
                          Best
                        </span>
                        <FormControl>
                          <Slider
                            min={0}
                            max={51}
                            step={1}
                            value={[field.value ?? 23]}
                            onValueChange={(val: number[]) =>
                              field.onChange(val[0])
                            }
                            className="flex-1"
                          />
                        </FormControl>
                        <span className="text-[10px] text-muted-foreground font-medium w-8">
                          Worst
                        </span>
                      </div>
                      <FormDescription className="text-[11px] text-right pt-1">
                        <Trans>Lower value = Higher Quality</Trans>
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </div>
            )}

            {/* Bitrate Control */}
            {(!isVideoCopy || !isAudioCopy) && (
              <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
                <div className="flex items-center gap-2 pb-2 border-border/40">
                  <Sliders className="w-4 h-4 text-cyan-500" />
                  <h3 className="font-semibold text-sm mr-auto">
                    <Trans>Target Bitrate</Trans>
                  </h3>
                </div>
                <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                  {!isVideoCopy && (
                    <FormField
                      control={control}
                      name={`${prefix}video_bitrate` as any}
                      render={({ field }) => (
                        <FormItem>
                          <FormLabel className="text-xs text-muted-foreground ml-1">
                            <Trans>Video Bitrate</Trans>
                          </FormLabel>
                          <FormControl>
                            <Input
                              className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                              placeholder="e.g. 5000k"
                              {...field}
                              value={field.value ?? ''}
                            />
                          </FormControl>
                          <FormMessage />
                        </FormItem>
                      )}
                    />
                  )}

                  {!isAudioCopy && (
                    <FormField
                      control={control}
                      name={`${prefix}audio_bitrate` as any}
                      render={({ field }) => (
                        <FormItem>
                          <FormLabel className="text-xs text-muted-foreground ml-1">
                            <Trans>Audio Bitrate</Trans>
                          </FormLabel>
                          <FormControl>
                            <Input
                              className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                              placeholder="e.g. 192k"
                              {...field}
                              value={field.value ?? ''}
                            />
                          </FormControl>
                          <FormMessage />
                        </FormItem>
                      )}
                    />
                  )}
                </div>
              </div>
            )}
          </TabsContent>

          <TabsContent
            value="video"
            className="space-y-6 focus-visible:outline-none focus-visible:ring-0"
          >
            {/* Dimensions & Filters */}
            {!isVideoCopy && (
              <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
                <div className="flex items-center gap-2 pb-2 border-b border-border/40">
                  <Box className="w-4 h-4 text-orange-500" />
                  <h3 className="font-semibold text-sm mr-auto">
                    <Trans>Dimensions & Filters</Trans>
                  </h3>
                </div>
                <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                  <FormField
                    control={control}
                    name={`${prefix}resolution` as any}
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel className="text-xs text-muted-foreground ml-1">
                          <Trans>Resolution (Scale)</Trans>
                        </FormLabel>
                        <FormControl>
                          <Input
                            className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                            placeholder="e.g. 1920:1080"
                            {...field}
                            value={field.value ?? ''}
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />

                  <FormField
                    control={control}
                    name={`${prefix}fps` as any}
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel className="text-xs text-muted-foreground ml-1">
                          <Trans>Framerate (FPS)</Trans>
                        </FormLabel>
                        <FormControl>
                          <Input
                            className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                            type="number"
                            placeholder="e.g. 60"
                            {...field}
                            value={field.value ?? ''}
                            onChange={(e) =>
                              field.onChange(parseFloat(e.target.value))
                            }
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />

                  <FormField
                    control={control}
                    name={`${prefix}video_filter` as any}
                    render={({ field }) => (
                      <FormItem className="col-span-1 md:col-span-2">
                        <FormLabel className="text-xs text-muted-foreground ml-1">
                          <Trans>FFmpeg Video Filter</Trans>
                        </FormLabel>
                        <FormControl>
                          <Input
                            className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-xs"
                            placeholder="e.g. hflip,noise=alls=20:allf=t+u"
                            {...field}
                            value={field.value ?? ''}
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                </div>
              </div>
            )}

            {/* Trimming Timeline */}
            <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
              <div className="flex items-center gap-2 pb-2 border-b border-border/40">
                <Timer className="w-4 h-4 text-indigo-500" />
                <h3 className="font-semibold text-sm mr-auto">
                  <Trans>Trimming & Time</Trans>
                </h3>
              </div>
              <div className="flex flex-col md:flex-row gap-4 items-end">
                <div className="flex-1 w-full relative">
                  <div className="absolute left-3 top-9 bottom-0 w-px bg-border/50 md:hidden"></div>
                  <FormField
                    control={control}
                    name={`${prefix}start_time` as any}
                    render={({ field }) => (
                      <FormItem>
                        <div className="flex items-center gap-2 mb-1.5">
                          <Play className="w-3.5 h-3.5 text-muted-foreground" />
                          <FormLabel className="text-xs text-muted-foreground">
                            <Trans>Start Time</Trans>
                          </FormLabel>
                        </div>
                        <FormControl>
                          <InputWithUnit
                            unitType="duration"
                            value={field.value}
                            onChange={(val: number | null) =>
                              field.onChange(val ?? 0)
                            }
                            className="bg-background/50"
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                </div>
                <div className="hidden md:flex h-11 items-center pb-2 text-muted-foreground/30">
                  <span className="text-xl">→</span>
                </div>
                <div className="flex-1 w-full">
                  <FormField
                    control={control}
                    name={`${prefix}duration` as any}
                    render={({ field }) => (
                      <FormItem>
                        <div className="flex items-center gap-2 mb-1.5">
                          <Timer className="w-3.5 h-3.5 text-muted-foreground" />
                          <FormLabel className="text-xs text-muted-foreground">
                            <Trans>Duration</Trans>
                          </FormLabel>
                        </div>
                        <FormControl>
                          <InputWithUnit
                            unitType="duration"
                            value={field.value}
                            onChange={(val: number | null) =>
                              field.onChange(val ?? 0)
                            }
                            className="bg-background/50"
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                </div>
                <div className="hidden md:flex h-11 items-center pb-2 text-muted-foreground/30">
                  <span className="text-xl">→</span>
                </div>
                <div className="flex-1 w-full">
                  <FormField
                    control={control}
                    name={`${prefix}end_time` as any}
                    render={({ field }) => (
                      <FormItem>
                        <div className="flex items-center gap-2 mb-1.5">
                          <Square className="w-3.5 h-3.5 text-muted-foreground" />
                          <FormLabel className="text-xs text-muted-foreground">
                            <Trans>End Time</Trans>
                          </FormLabel>
                        </div>
                        <FormControl>
                          <InputWithUnit
                            unitType="duration"
                            value={field.value}
                            onChange={(val: number | null) =>
                              field.onChange(val ?? 0)
                            }
                            className="bg-background/50"
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                </div>
              </div>
            </div>
          </TabsContent>

          <TabsContent
            value="audio"
            className="space-y-4 focus-visible:outline-none focus-visible:ring-0"
          >
            {!isAudioCopy && (
              <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
                <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
                  <MousePointer2 className="w-4 h-4 text-pink-500" />
                  <h3 className="font-semibold text-sm mr-auto">
                    <Trans>Audio Processing</Trans>
                  </h3>
                </div>
                <FormField
                  control={control}
                  name={`${prefix}audio_filter` as any}
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel className="text-xs text-muted-foreground ml-1">
                        <Trans>FFmpeg Audio Filter (-af)</Trans>
                      </FormLabel>
                      <FormControl>
                        <Input
                          className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-xs"
                          placeholder="e.g. volume=0.5"
                          {...field}
                          value={field.value ?? ''}
                        />
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </div>
            )}
          </TabsContent>

          <TabsContent
            value="advanced"
            className="space-y-6 focus-visible:outline-none focus-visible:ring-0"
          >
            <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
              {/* Hardware Accel & Flags */}
              <div className="space-y-4">
                <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
                  <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
                    <Settings2 className="w-4 h-4 text-gray-500" />
                    <h3 className="font-semibold text-sm mr-auto">
                      <Trans>Performance</Trans>
                    </h3>
                  </div>
                  <FormField
                    control={control}
                    name={`${prefix}hwaccel` as any}
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel className="text-xs text-muted-foreground ml-1">
                          <Trans>Hardware Acceleration</Trans>
                        </FormLabel>
                        <Select
                          onValueChange={field.onChange}
                          defaultValue={field.value}
                        >
                          <FormControl>
                            <SelectTrigger className="h-11 bg-background/50 border-border/50 focus:bg-background transition-colors rounded-lg">
                              <SelectValue placeholder="None" />
                            </SelectTrigger>
                          </FormControl>
                          <SelectContent>
                            <SelectItem value="none">None</SelectItem>
                            <SelectItem value="cuda">CUDA (NVIDIA)</SelectItem>
                            <SelectItem value="vaapi">
                              VAAPI (Intel/AMD)
                            </SelectItem>
                            <SelectItem value="qsv">QSV (Intel)</SelectItem>
                            <SelectItem value="videotoolbox">
                              VideoToolbox (Mac)
                            </SelectItem>
                          </SelectContent>
                        </Select>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                </div>
                <div className="space-y-3">
                  <FormField
                    control={control}
                    name={`${prefix}faststart` as any}
                    render={({ field }) => (
                      <FormItem
                        className={cn(
                          'flex flex-row items-center justify-between rounded-xl border border-border/40 p-4 shadow-sm bg-muted/10 transition-colors',
                          faststartSupported
                            ? 'hover:bg-muted/20'
                            : 'opacity-50',
                        )}
                      >
                        <div className="space-y-1">
                          <FormLabel className="text-sm font-medium">
                            <Trans>Web Optimize</Trans>
                          </FormLabel>
                          <FormDescription className="text-xs">
                            <Trans>
                              Use <code>-movflags +faststart</code>
                            </Trans>
                            {!faststartSupported && (
                              <span className="block text-destructive/70 mt-1">
                                <Trans>Only supported for MP4/MOV/M4V</Trans>
                              </span>
                            )}
                          </FormDescription>
                        </div>
                        <FormControl>
                          <Switch
                            checked={field.value}
                            onCheckedChange={field.onChange}
                            disabled={!faststartSupported}
                          />
                        </FormControl>
                      </FormItem>
                    )}
                  />
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
                            <Trans>Force overwrite if output exists</Trans>
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
                  <FormField
                    control={control}
                    name={`${prefix}remove_input_on_success` as any}
                    render={({ field }) => (
                      <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 p-4 shadow-sm bg-muted/10 transition-colors hover:bg-muted/20">
                        <div className="space-y-1">
                          <FormLabel className="text-sm font-medium">
                            <Trans>Remove Input on Success</Trans>
                          </FormLabel>
                          <FormDescription className="text-xs">
                            <Trans>
                              Delete original file after successful remux
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
              </div>

              {/* Custom Flags */}
              <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
                <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
                  <Sliders className="w-4 h-4 text-amber-500" />
                  <h3 className="font-semibold text-sm mr-auto">
                    <Trans>Custom Flags</Trans>
                  </h3>
                </div>
                <FormField
                  control={control}
                  name={`${prefix}input_options` as any}
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel className="text-xs text-muted-foreground ml-1">
                        <Trans>Input Options (-i flags)</Trans>
                      </FormLabel>
                      <FormControl>
                        <ListInput
                          value={field.value || []}
                          onChange={field.onChange}
                          placeholder={i18n._(msg`--flag value`)}
                        />
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />

                <FormField
                  control={control}
                  name={`${prefix}output_options` as any}
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel className="text-xs text-muted-foreground ml-1">
                        <Trans>Output Options</Trans>
                      </FormLabel>
                      <FormControl>
                        <ListInput
                          value={field.value || []}
                          onChange={field.onChange}
                          placeholder={i18n._(msg`--flag value`)}
                        />
                      </FormControl>
                      <FormDescription className="text-xs">
                        <Trans>Appended before output filename</Trans>
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </div>

              {/* Stream & Metadata */}
              <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-6">
                <div className="flex items-center gap-2 pb-2 border-b border-border/40">
                  <Layers className="w-4 h-4 text-blue-500" />
                  <h3 className="font-semibold text-sm mr-auto">
                    <Trans>Streams & Metadata</Trans>
                  </h3>
                </div>

                <FormField
                  control={control}
                  name={`${prefix}map_streams` as any}
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel className="text-xs text-muted-foreground ml-1">
                        <Trans>Stream Mapping (-map)</Trans>
                      </FormLabel>
                      <FormControl>
                        <ListInput
                          value={field.value || []}
                          onChange={field.onChange}
                          placeholder={i18n._(msg`0:v:0`)}
                        />
                      </FormControl>
                      <FormDescription className="text-xs">
                        <Trans>e.g. 0:v:0, 0:a:0</Trans>
                      </FormDescription>
                      <FormMessage />
                    </FormItem>
                  )}
                />

                <FormField
                  control={control}
                  name={`${prefix}metadata` as any}
                  render={({ field }) => (
                    <FormItem>
                      <div className="flex items-center justify-between mb-2">
                        <FormLabel className="text-xs text-muted-foreground ml-1">
                          <Trans>Metadata Tags</Trans>
                        </FormLabel>
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          className="h-7 px-2 text-[10px] text-primary hover:text-primary hover:bg-primary/10"
                          onClick={() => {
                            const current = field.value || [];
                            field.onChange([...current, ['', '']]);
                          }}
                        >
                          <Plus className="w-3 h-3 mr-1" />
                          <Trans>Add Tag</Trans>
                        </Button>
                      </div>
                      <div className="space-y-2">
                        {(field.value || []).map(
                          ([key, value]: [string, string], index: number) => (
                            <div
                              key={index}
                              className="flex gap-2 items-center group animate-in fade-in slide-in-from-top-1 duration-200"
                            >
                              <div className="relative flex-1 group/input">
                                <Tags className="absolute left-2.5 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-muted-foreground/40 group-focus-within/input:text-primary/50 transition-colors" />
                                <Input
                                  placeholder={i18n._(msg`Key`)}
                                  value={key}
                                  onChange={(e) => {
                                    const newValue = [...(field.value || [])];
                                    newValue[index] = [e.target.value, value];
                                    field.onChange(newValue);
                                  }}
                                  className="h-9 pl-8 bg-background/50 border-border/50 focus:bg-background text-xs"
                                />
                              </div>
                              <Input
                                placeholder={i18n._(msg`Value`)}
                                value={value}
                                onChange={(e) => {
                                  const newValue = [...(field.value || [])];
                                  newValue[index] = [key, e.target.value];
                                  field.onChange(newValue);
                                }}
                                className="flex-1 h-9 bg-background/50 border-border/50 focus:bg-background text-xs"
                              />
                              <Button
                                type="button"
                                variant="ghost"
                                size="icon"
                                className="h-9 w-9 shrink-0 text-muted-foreground/30 hover:text-destructive hover:bg-destructive/10"
                                onClick={() => {
                                  const newValue = [...(field.value || [])];
                                  newValue.splice(index, 1);
                                  field.onChange(newValue);
                                }}
                              >
                                <Trash2 className="h-4 w-4" />
                              </Button>
                            </div>
                          ),
                        )}
                        {(!field.value || field.value.length === 0) && (
                          <div className="text-[11px] text-muted-foreground/50 text-center py-4 border border-dashed border-border/30 rounded-lg">
                            <Trans>No metadata tags added</Trans>
                          </div>
                        )}
                      </div>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </div>
            </div>
          </TabsContent>
        </div>
      </Tabs>
    </motion.div>
  );
});
