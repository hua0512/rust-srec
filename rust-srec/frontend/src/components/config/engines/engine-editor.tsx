import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { t } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { motion, AnimatePresence } from 'motion/react';
import {
  Form,
  FormControl,
  FormDescription,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import {
  Tag,
  Terminal,
  Radio,
  Database,
  Save,
  Loader2,
  ArrowLeft,
} from 'lucide-react';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  CardDescription,
} from '@/components/ui/card';
import {
  EngineConfigSchema,
  CreateEngineRequestSchema,
  UpdateEngineRequestSchema,
  FfmpegConfigSchema,
  StreamlinkConfigSchema,
  MesioConfigSchema,
} from '@/api/schemas';
import { createEngine, updateEngine } from '@/server/functions';
import { FfmpegForm } from './forms/ffmpeg-form';
import { StreamlinkForm } from './forms/streamlink-form';
import { MesioForm } from './forms/mesio-form';
import { useEffect } from 'react';
import { Link } from '@tanstack/react-router';
import { cn } from '@/lib/utils';
import { Badge } from '@/components/ui/badge';

interface EngineEditorProps {
  engine?: z.infer<typeof EngineConfigSchema>;
  onSuccess: () => void;
}

export function EngineEditor({ engine, onSuccess }: EngineEditorProps) {
  const isEdit = !!engine;
  const queryClient = useQueryClient();

  const form = useForm<z.infer<typeof CreateEngineRequestSchema>>({
    resolver: zodResolver(CreateEngineRequestSchema),
    defaultValues: {
      name: '',
      engine_type: 'FFMPEG',
      config: FfmpegConfigSchema.parse({}),
    },
  });

  useEffect(() => {
    if (engine) {
      form.reset({
        name: engine.name,
        engine_type: engine.engine_type,
        config: engine.config as Record<string, unknown>,
      });
    }
  }, [engine, form]);

  const engineType = form.watch('engine_type');

  const createMutation = useMutation({
    mutationFn: (data: z.infer<typeof CreateEngineRequestSchema>) =>
      createEngine({ data }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['engines'] });
      onSuccess();
    },
    onError: (error: Error) => {
      console.error('Failed to create engine', error);
    },
  });

  const updateMutation = useMutation({
    mutationFn: (data: z.infer<typeof UpdateEngineRequestSchema>) =>
      updateEngine({ data: { id: engine!.id, data } }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['engines'] });
      onSuccess();
    },
  });

  const onSubmit = (data: z.infer<typeof CreateEngineRequestSchema>) => {
    if (isEdit) {
      updateMutation.mutate(data);
    } else {
      createMutation.mutate(data);
    }
  };

  const isSubmitting = createMutation.isPending || updateMutation.isPending;

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3 }}
      className="space-y-8"
    >
      {/* Header Section with Breadcrumbs */}
      <div className="flex flex-col space-y-4 md:flex-row md:items-center md:justify-between md:space-y-0">
        <div className="space-y-1.5">
          <div className="flex items-center gap-4">
            <Link to="/config/engines">
              <Button
                variant="outline"
                size="icon"
                className="h-10 w-10 rounded-full border-border/50 bg-background/50 backdrop-blur-sm hover:bg-background/80 hover:scale-105 transition-all"
              >
                <ArrowLeft className="h-5 w-5" />
              </Button>
            </Link>
            <div>
              <h1 className="text-3xl font-bold tracking-tight bg-gradient-to-r from-foreground to-foreground/70 bg-clip-text text-transparent">
                {isEdit ? t`Edit Engine` : t`Create Engine`}
              </h1>
              <p className="text-muted-foreground text-sm font-medium">
                {isEdit ? (
                  <Trans>Configure download engine parameters</Trans>
                ) : (
                  <Trans>Set up a new download tool configuration</Trans>
                )}
              </p>
            </div>
          </div>
        </div>

        <div className="flex items-center gap-3">
          <Badge
            variant="outline"
            className="h-8 px-3 py-0 text-sm font-medium bg-background/50 backdrop-blur-sm border-border/50"
          >
            {isEdit ? (
              <Trans>Editing Mode</Trans>
            ) : (
              <Trans>Creation Mode</Trans>
            )}
          </Badge>
        </div>
      </div>

      <Form {...form}>
        <form onSubmit={form.handleSubmit(onSubmit)} className="space-y-8">
          {/* Basic Info Card */}
          <Card className="border-border/40 bg-gradient-to-br from-card to-card/50 backdrop-blur-sm shadow-sm hover:shadow-md transition-shadow duration-500">
            <CardHeader className="pb-4">
              <div className="flex items-center gap-3">
                <div className="p-2 rounded-lg bg-primary/10 text-primary ring-1 ring-primary/20">
                  <Tag className="w-5 h-5" />
                </div>
                <div>
                  <CardTitle className="text-lg">
                    <Trans>Basic Information</Trans>
                  </CardTitle>
                  <CardDescription>
                    <Trans>Identity and type of the engine.</Trans>
                  </CardDescription>
                </div>
              </div>
            </CardHeader>
            <CardContent className="grid gap-8 md:grid-cols-2">
              <FormField
                control={form.control}
                name="name"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel className="text-xs uppercase tracking-wider text-muted-foreground font-semibold">
                      <Trans>Engine Name</Trans>
                    </FormLabel>
                    <FormControl>
                      <Input
                        className="h-10 bg-background/50 border-input/50 focus:border-primary/50 transition-colors"
                        placeholder={t`e.g. High Quality Streamlink`}
                        {...field}
                      />
                    </FormControl>
                    <FormDescription>
                      <Trans>
                        A unique and descriptive name for this configuration.
                      </Trans>
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />

              <FormField
                control={form.control}
                name="engine_type"
                render={({ field }) => (
                  <FormItem>
                    <FormLabel className="text-xs uppercase tracking-wider text-muted-foreground font-semibold">
                      <Trans>Engine Type</Trans>
                    </FormLabel>
                    <Select
                      onValueChange={(value) => {
                        field.onChange(value);
                        // Reset config to defaults of new type
                        if (value === 'FFMPEG')
                          form.setValue(
                            'config',
                            FfmpegConfigSchema.parse({}) as any,
                          );
                        if (value === 'STREAMLINK')
                          form.setValue(
                            'config',
                            StreamlinkConfigSchema.parse({}) as any,
                          );
                        if (value === 'MESIO')
                          form.setValue(
                            'config',
                            MesioConfigSchema.parse({}) as any,
                          );
                      }}
                      defaultValue={field.value}
                    >
                      <FormControl>
                        <SelectTrigger className="h-10 bg-background/50 border-input/50 focus:border-primary/50 transition-colors">
                          <SelectValue placeholder={t`Select type`} />
                        </SelectTrigger>
                      </FormControl>
                      <SelectContent>
                        <SelectItem value="FFMPEG">
                          <div className="flex items-center gap-2">
                            <Terminal className="w-4 h-4 text-emerald-500" />
                            <span className="font-medium">FFmpeg</span>
                          </div>
                        </SelectItem>
                        <SelectItem value="STREAMLINK">
                          <div className="flex items-center gap-2">
                            <Radio className="w-4 h-4 text-sky-500" />
                            <span className="font-medium">Streamlink</span>
                          </div>
                        </SelectItem>
                        <SelectItem value="MESIO">
                          <div className="flex items-center gap-2">
                            <Database className="w-4 h-4 text-indigo-500" />
                            <span className="font-medium">Mesio</span>
                          </div>
                        </SelectItem>
                      </SelectContent>
                    </Select>
                    <FormDescription>
                      <Trans>The underlying tool used for downloading.</Trans>
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
            </CardContent>
          </Card>

          {/* Configuration Card */}
          <Card className="border-border/40 bg-gradient-to-br from-card to-card/50 backdrop-blur-sm shadow-sm hover:shadow-md transition-shadow duration-500">
            <CardHeader className="border-b border-border/40 bg-muted/20 pb-4">
              <div className="flex flex-row items-center gap-3">
                <div className="p-2 rounded-lg bg-primary/10 text-primary ring-1 ring-primary/20">
                  {engineType === 'FFMPEG' && <Terminal className="w-5 h-5" />}
                  {engineType === 'STREAMLINK' && <Radio className="w-5 h-5" />}
                  {engineType === 'MESIO' && <Database className="w-5 h-5" />}
                </div>
                <div className="space-y-0.5">
                  <CardTitle className="text-lg flex items-center gap-2">
                    {engineType === 'FFMPEG'
                      ? 'FFmpeg'
                      : engineType === 'STREAMLINK'
                        ? 'Streamlink'
                        : 'Mesio'}
                    <Trans>Settings</Trans>
                  </CardTitle>
                  <CardDescription>
                    <Trans>
                      Configure granular options for the selected engine.
                    </Trans>
                  </CardDescription>
                </div>
              </div>
            </CardHeader>
            <CardContent className="pt-6">
              {engineType === 'FFMPEG' && <FfmpegForm control={form.control} />}
              {engineType === 'STREAMLINK' && (
                <StreamlinkForm control={form.control} />
              )}
              {engineType === 'MESIO' && <MesioForm control={form.control} />}
            </CardContent>
          </Card>

          <AnimatePresence>
            {form.formState.isDirty && (
              <motion.div
                initial={{ opacity: 0, y: 20, scale: 0.9 }}
                animate={{ opacity: 1, y: 0, scale: 1 }}
                exit={{ opacity: 0, y: 20, scale: 0.9 }}
                transition={{ duration: 0.2 }}
                className="fixed bottom-6 right-6 md:bottom-10 md:right-10 z-50"
              >
                <Button
                  type="submit"
                  disabled={isSubmitting}
                  size="lg"
                  className={cn(
                    'h-14 px-8 rounded-full font-semibold shadow-2xl shadow-primary/30 transition-all duration-300',
                    isSubmitting
                      ? 'opacity-50 grayscale scale-95'
                      : 'bg-gradient-to-r from-primary to-primary/90 hover:scale-105 active:scale-95 hover:shadow-primary/50',
                  )}
                >
                  {isSubmitting ? (
                    <Loader2 className="w-5 h-5 mr-2 animate-spin" />
                  ) : (
                    <Save className="w-5 h-5 mr-2" />
                  )}
                  {isEdit ? t`Save Changes` : t`Create Engine`}
                </Button>
              </motion.div>
            )}
          </AnimatePresence>
          {/* Add padding at bottom to prevent floating button from covering content */}
          <div className="h-24" />
        </form>
      </Form>
    </motion.div>
  );
}
