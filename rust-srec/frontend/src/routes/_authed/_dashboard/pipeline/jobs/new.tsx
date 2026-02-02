import { createFileRoute } from '@tanstack/react-router';
import { useMutation, useQueryClient, useQuery } from '@tanstack/react-query';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { motion } from 'motion/react';
import { useLingui } from '@lingui/react';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { toast } from 'sonner';
import { z } from 'zod';
import {
  ChevronLeft,
  Loader2,
  Rocket,
  FileText,
  User,
  Video,
  Info,
} from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Card } from '@/components/ui/card';
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  createPipelineJob,
  listSessions,
  listStreamers,
} from '@/server/functions';
import { PipelineWorkflowEditor } from '@/components/pipeline/workflows/pipeline-workflow-editor';
import { DagStepDefinition } from '@/api/schemas';

const createPipelineSchema = z.object({
  name: z.string().min(1, msg`Pipeline name is required`),
  session_id: z.string().min(1, msg`Session ID is required`),
  streamer_id: z.string().min(1, msg`Streamer ID is required`),
  input_paths: z
    .array(z.string())
    .min(1, msg`At least one input path is required`),
  steps: z.array(z.any()).min(1, msg`Add at least one step`),
});

type CreatePipelineForm = z.infer<typeof createPipelineSchema>;

export const Route = createFileRoute('/_authed/_dashboard/pipeline/jobs/new')({
  component: CreatePipelineJobPage,
});

function CreatePipelineJobPage() {
  const navigate = Route.useNavigate();
  const queryClient = useQueryClient();
  const { i18n } = useLingui();

  const form = useForm<CreatePipelineForm>({
    resolver: zodResolver(createPipelineSchema),
    defaultValues: {
      name: '',
      session_id: '',
      streamer_id: '',
      input_paths: [],
      steps: [],
    },
  });

  const { data: sessionsData } = useQuery({
    queryKey: ['sessions', 'list', { limit: 100 }],
    queryFn: () => listSessions({ data: { limit: 100 } }),
  });

  const { data: streamersData } = useQuery({
    queryKey: ['streamers', 'list', { limit: 100 }],
    queryFn: () => listStreamers({ data: { limit: 100 } }),
  });

  const createMutation = useMutation({
    mutationFn: (values: CreatePipelineForm) => {
      const formattedPayload = {
        session_id: values.session_id,
        streamer_id: values.streamer_id,
        input_paths: values.input_paths,
        dag: {
          name: values.name,
          steps: values.steps,
        },
      };
      return createPipelineJob({ data: formattedPayload });
    },
    onSuccess: () => {
      toast.success(i18n._(msg`Pipeline job created successfully`));
      void queryClient.invalidateQueries({
        queryKey: ['pipeline', 'pipelines'],
      });
      void navigate({ to: '/pipeline/jobs' });
    },
    onError: (error: any) => {
      toast.error(error?.message || i18n._(msg`Failed to create pipeline job`));
    },
  });

  const onSubmit = (values: CreatePipelineForm) => {
    createMutation.mutate(values);
  };

  return (
    <div className="relative min-h-[calc(100vh-4rem)] p-4 md:p-8 overflow-hidden">
      {/* Background decoration */}
      <div className="fixed inset-0 -z-10 pointer-events-none">
        <div className="absolute top-1/4 left-1/4 w-[500px] h-[500px] bg-primary/5 rounded-full blur-[120px] animate-pulse" />
        <div className="absolute bottom-1/4 right-1/4 w-[400px] h-[400px] bg-blue-500/5 rounded-full blur-[100px] animate-pulse [animation-delay:1s]" />
      </div>

      <motion.div
        initial={{ opacity: 0, y: 20 }}
        animate={{ opacity: 1, y: 0 }}
        className="max-w-7xl mx-auto space-y-8"
      >
        {/* Header */}
        <div className="flex flex-col gap-4 sm:flex-row sm:items-center justify-between">
          <div className="space-y-1">
            <Button
              variant="ghost"
              size="sm"
              className="-ml-2 h-8 gap-1 text-muted-foreground hover:text-foreground"
              onClick={() => navigate({ to: '/pipeline/jobs' })}
            >
              <ChevronLeft className="h-4 w-4" />
              <Trans>Back to Jobs</Trans>
            </Button>
            <div className="flex items-center gap-3">
              <div className="p-2 rounded-xl bg-primary/10 text-primary">
                <Rocket className="h-6 w-6" />
              </div>
              <h1 className="text-2xl font-bold tracking-tight">
                <Trans>Create New Pipeline Job</Trans>
              </h1>
            </div>
            <p className="text-muted-foreground">
              <Trans>Configure and launch a manual processing pipeline.</Trans>
            </p>
          </div>

          <div className="flex items-center gap-3">
            <Button
              variant="outline"
              onClick={() => navigate({ to: '/pipeline/jobs' })}
              disabled={createMutation.isPending}
            >
              <Trans>Cancel</Trans>
            </Button>
            <Button
              onClick={form.handleSubmit(onSubmit)}
              disabled={createMutation.isPending}
              className="gap-2 shadow-lg shadow-primary/20"
            >
              {createMutation.isPending ? (
                <>
                  <Loader2 className="h-4 w-4 animate-spin" />
                  <Trans>Creating...</Trans>
                </>
              ) : (
                <>
                  <Rocket className="h-4 w-4" />
                  <Trans>Launch Pipeline</Trans>
                </>
              )}
            </Button>
          </div>
        </div>

        <Form {...form}>
          <form className="grid grid-cols-1 lg:grid-cols-12 gap-8">
            {/* Left Column: Configuration */}
            <div className="lg:col-span-4 space-y-6">
              <Card className="p-6 border-white/5 bg-background/40 backdrop-blur-xl dark:bg-card/30 shadow-2xl relative overflow-hidden group">
                <div className="absolute inset-0 bg-gradient-to-br from-primary/5 via-transparent to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-500" />

                <div className="relative z-10 space-y-6">
                  <div className="flex items-center gap-2 mb-2 text-primary">
                    <Info className="h-4 w-4" />
                    <h2 className="text-sm font-semibold uppercase tracking-wider">
                      <Trans>Job Configuration</Trans>
                    </h2>
                  </div>

                  <FormField
                    control={form.control}
                    name="name"
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel className="flex items-center gap-2">
                          <FileText className="h-3.5 w-3.5 text-muted-foreground" />
                          <Trans>Pipeline Name</Trans>
                        </FormLabel>
                        <FormControl>
                          <Input
                            placeholder={i18n._(msg`My Archiving Workflow`)}
                            {...field}
                            className="bg-background/50 border-white/10 focus:ring-primary/20"
                          />
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />

                  <FormField
                    control={form.control}
                    name="streamer_id"
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel className="flex items-center gap-2">
                          <User className="h-3.5 w-3.5 text-muted-foreground" />
                          <Trans>Streamer</Trans>
                        </FormLabel>
                        <Select
                          onValueChange={field.onChange}
                          defaultValue={field.value}
                        >
                          <FormControl>
                            <SelectTrigger className="bg-background/50 border-white/10">
                              <SelectValue
                                placeholder={i18n._(msg`Select a streamer`)}
                              />
                            </SelectTrigger>
                          </FormControl>
                          <SelectContent className="backdrop-blur-xl bg-background/90">
                            {streamersData?.items.map((streamer) => (
                              <SelectItem key={streamer.id} value={streamer.id}>
                                {streamer.name}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                        <FormMessage />
                      </FormItem>
                    )}
                  />

                  <FormField
                    control={form.control}
                    name="session_id"
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel className="flex items-center gap-2">
                          <FileText className="h-3.5 w-3.5 text-muted-foreground" />
                          <Trans>Session</Trans>
                        </FormLabel>
                        <Select
                          onValueChange={field.onChange}
                          defaultValue={field.value}
                        >
                          <FormControl>
                            <SelectTrigger className="bg-background/50 border-white/10">
                              <SelectValue
                                placeholder={i18n._(msg`Select a session`)}
                              />
                            </SelectTrigger>
                          </FormControl>
                          <SelectContent className="backdrop-blur-xl bg-background/90">
                            {sessionsData?.items.map((session) => (
                              <SelectItem key={session.id} value={session.id}>
                                {session.title || session.id.slice(0, 8)}
                              </SelectItem>
                            ))}
                          </SelectContent>
                        </Select>
                        <FormMessage />
                      </FormItem>
                    )}
                  />

                  <FormField
                    control={form.control}
                    name="input_paths"
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel className="flex items-center gap-2">
                          <Video className="h-3.5 w-3.5 text-muted-foreground" />
                          <Trans>Input Paths (comma separated)</Trans>
                        </FormLabel>
                        <FormControl>
                          <Input
                            placeholder={i18n._(msg`C:\path1.flv,C:\path2.flv`)}
                            value={field.value.join(',')}
                            onChange={(e) =>
                              field.onChange(
                                e.target.value
                                  .split(',')
                                  .map((s) => s.trim())
                                  .filter((s) => s !== ''),
                              )
                            }
                            className="bg-background/50 border-white/10"
                          />
                        </FormControl>
                        <p className="text-[0.8rem] text-muted-foreground mt-1">
                          <Trans>Separate multiple paths with commas.</Trans>
                        </p>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                </div>
              </Card>
            </div>

            {/* Right Column: Workflow Editor */}
            <div className="lg:col-span-8">
              <Card className="p-6 border-white/5 bg-background/40 backdrop-blur-xl dark:bg-card/30 shadow-2xl relative overflow-hidden h-full">
                <div className="relative z-10 flex flex-col h-full">
                  <PipelineWorkflowEditor
                    steps={form.watch('steps') as DagStepDefinition[]}
                    onChange={(steps) =>
                      form.setValue('steps', steps, {
                        shouldDirty: true,
                        shouldValidate: true,
                      })
                    }
                  />
                  {form.formState.errors.steps && (
                    <p className="text-sm font-medium text-destructive mt-2">
                      {form.formState.errors.steps.message}
                    </p>
                  )}
                </div>
              </Card>
            </div>
          </form>
        </Form>
      </motion.div>
    </div>
  );
}
