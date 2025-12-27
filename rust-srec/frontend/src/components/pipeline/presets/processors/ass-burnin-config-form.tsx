import { Trans } from '@lingui/macro';
import {
    FormField,
    FormItem,
    FormLabel,
    FormControl,
    FormMessage,
    FormDescription,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from '@/components/ui/select';
import { ProcessorConfigFormProps } from './common-props';
import { AssBurninConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { motion } from 'motion/react';
import { Video, Type, Settings, Trash2 } from 'lucide-react';

type AssBurninConfig = z.infer<typeof AssBurninConfigSchema>;

export function AssBurninConfigForm({
    control,
    pathPrefix,
}: ProcessorConfigFormProps<AssBurninConfig>) {
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
            <div className="space-y-6">
                {/* Encoding Settings */}
                <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
                    <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
                        <Video className="w-4 h-4 text-blue-500" />
                        <h3 className="font-semibold text-sm mr-auto">
                            <Trans>Encoding Settings</Trans>
                        </h3>
                    </div>

                    <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                        <FormField
                            control={control}
                            name={`${prefix}video_codec` as any}
                            render={({ field }) => (
                                <FormItem>
                                    <FormLabel className="text-xs text-muted-foreground ml-1">
                                        <Trans>Video Codec</Trans>
                                    </FormLabel>
                                    <FormControl>
                                        <Input
                                            className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                                            placeholder="libx264"
                                            {...field}
                                        />
                                    </FormControl>
                                    <FormMessage />
                                </FormItem>
                            )}
                        />

                        <FormField
                            control={control}
                            name={`${prefix}audio_codec` as any}
                            render={({ field }) => (
                                <FormItem>
                                    <FormLabel className="text-xs text-muted-foreground ml-1">
                                        <Trans>Audio Codec</Trans>
                                    </FormLabel>
                                    <FormControl>
                                        <Input
                                            className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                                            placeholder="copy"
                                            {...field}
                                        />
                                    </FormControl>
                                    <FormMessage />
                                </FormItem>
                            )}
                        />

                        <FormField
                            control={control}
                            name={`${prefix}crf` as any}
                            render={({ field }) => (
                                <FormItem>
                                    <FormLabel className="text-xs text-muted-foreground ml-1">
                                        <Trans>CRF (0-51)</Trans>
                                    </FormLabel>
                                    <FormControl>
                                        <Input
                                            className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                                            type="number"
                                            min={0}
                                            max={51}
                                            {...field}
                                            onChange={(e) => field.onChange(parseInt(e.target.value))}
                                        />
                                    </FormControl>
                                    <FormMessage />
                                </FormItem>
                            )}
                        />

                        <FormField
                            control={control}
                            name={`${prefix}preset` as any}
                            render={({ field }) => (
                                <FormItem>
                                    <FormLabel className="text-xs text-muted-foreground ml-1">
                                        <Trans>Encoder Preset</Trans>
                                    </FormLabel>
                                    <Select onValueChange={field.onChange} defaultValue={field.value}>
                                        <FormControl>
                                            <SelectTrigger className="h-11 bg-background/50 border-border/50 focus:ring-primary/20 rounded-lg">
                                                <SelectValue placeholder="veryfast" />
                                            </SelectTrigger>
                                        </FormControl>
                                        <SelectContent>
                                            <SelectItem value="ultrafast">ultrafast</SelectItem>
                                            <SelectItem value="superfast">superfast</SelectItem>
                                            <SelectItem value="veryfast">veryfast</SelectItem>
                                            <SelectItem value="faster">faster</SelectItem>
                                            <SelectItem value="fast">fast</SelectItem>
                                            <SelectItem value="medium">medium</SelectItem>
                                            <SelectItem value="slow">slow</SelectItem>
                                            <SelectItem value="slower">slower</SelectItem>
                                            <SelectItem value="veryslow">veryslow</SelectItem>
                                        </SelectContent>
                                    </Select>
                                    <FormMessage />
                                </FormItem>
                            )}
                        />
                    </div>
                </div>

                {/* Subtitle Matching */}
                <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
                    <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
                        <Type className="w-4 h-4 text-indigo-500" />
                        <h3 className="font-semibold text-sm mr-auto">
                            <Trans>Subtitle Matching</Trans>
                        </h3>
                    </div>

                    <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                        <FormField
                            control={control}
                            name={`${prefix}match_strategy` as any}
                            render={({ field }) => (
                                <FormItem>
                                    <FormLabel className="text-xs text-muted-foreground ml-1">
                                        <Trans>Match Strategy</Trans>
                                    </FormLabel>
                                    <Select onValueChange={field.onChange} defaultValue={field.value}>
                                        <FormControl>
                                            <SelectTrigger className="h-11 bg-background/50 border-border/50 focus:ring-primary/20 rounded-lg">
                                                <SelectValue placeholder="Select strategy" />
                                            </SelectTrigger>
                                        </FormControl>
                                        <SelectContent>
                                            <SelectItem value="manifest">
                                                <Trans>Manifest (Pair by Order)</Trans>
                                            </SelectItem>
                                            <SelectItem value="stem">
                                                <Trans>Stem (Filename Match)</Trans>
                                            </SelectItem>
                                        </SelectContent>
                                    </Select>
                                    <FormDescription className="text-[10px] ml-1">
                                        <Trans>How to pair video files with .ass subtitles</Trans>
                                    </FormDescription>
                                    <FormMessage />
                                </FormItem>
                            )}
                        />

                        <FormField
                            control={control}
                            name={`${prefix}require_ass` as any}
                            render={({ field }) => (
                                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm bg-background/50 self-end h-11">
                                    <div className="space-y-0.5">
                                        <FormLabel className="text-xs">
                                            <Trans>Require Subtitles</Trans>
                                        </FormLabel>
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

                {/* Advanced Settings */}
                <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
                    <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
                        <Settings className="w-4 h-4 text-slate-500" />
                        <h3 className="font-semibold text-sm mr-auto">
                            <Trans>Advanced Settings</Trans>
                        </h3>
                    </div>

                    <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                        <FormField
                            control={control}
                            name={`${prefix}ffmpeg_path` as any}
                            render={({ field }) => (
                                <FormItem>
                                    <FormLabel className="text-xs text-muted-foreground ml-1">
                                        <Trans>FFmpeg Path</Trans>
                                    </FormLabel>
                                    <FormControl>
                                        <Input
                                            className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                                            placeholder="ffmpeg"
                                            {...field}
                                        />
                                    </FormControl>
                                    <FormMessage />
                                </FormItem>
                            )}
                        />

                        <FormField
                            control={control}
                            name={`${prefix}fonts_dir` as any}
                            render={({ field }) => (
                                <FormItem>
                                    <FormLabel className="text-xs text-muted-foreground ml-1">
                                        <Trans>Fonts Directory</Trans>
                                    </FormLabel>
                                    <FormControl>
                                        <Input
                                            className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm"
                                            placeholder="/usr/share/fonts"
                                            {...field}
                                        />
                                    </FormControl>
                                    <FormMessage />
                                </FormItem>
                            )}
                        />
                    </div>

                    <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                        <FormField
                            control={control}
                            name={`${prefix}passthrough_inputs` as any}
                            render={({ field }) => (
                                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm bg-background/50">
                                    <div className="space-y-0.5">
                                        <FormLabel className="text-xs">
                                            <Trans>Passthrough Inputs</Trans>
                                        </FormLabel>
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
                            name={`${prefix}exclude_ass_from_passthrough` as any}
                            render={({ field }) => (
                                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm bg-background/50">
                                    <div className="space-y-0.5">
                                        <FormLabel className="text-xs">
                                            <Trans>Exclude ASS from Passthrough</Trans>
                                        </FormLabel>
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

                {/* Cleanup Settings */}
                <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
                    <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
                        <Trash2 className="w-4 h-4 text-red-500" />
                        <h3 className="font-semibold text-sm mr-auto">
                            <Trans>Cleanup</Trans>
                        </h3>
                    </div>

                    <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                        <FormField
                            control={control}
                            name={`${prefix}delete_source_videos_on_success` as any}
                            render={({ field }) => (
                                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm bg-background/50 border-red-500/20">
                                    <div className="space-y-0.5">
                                        <FormLabel className="text-xs text-red-600 dark:text-red-400">
                                            <Trans>Delete Source Videos</Trans>
                                        </FormLabel>
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
                            name={`${prefix}delete_source_ass_on_success` as any}
                            render={({ field }) => (
                                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm bg-background/50 border-red-500/20">
                                    <div className="space-y-0.5">
                                        <FormLabel className="text-xs text-red-600 dark:text-red-400">
                                            <Trans>Delete Source ASS</Trans>
                                        </FormLabel>
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
            </div>
        </motion.div>
    );
}
