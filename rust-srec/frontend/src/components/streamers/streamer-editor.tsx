import { ReactNode, useState } from 'react';
import { motion } from 'motion/react';
import { useBlocker, useNavigate } from '@tanstack/react-router';
import { Trans } from '@lingui/react/macro';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { toast } from 'sonner';
import { useQuery } from '@tanstack/react-query';
import {
  Activity,
  ArrowLeft,
  ArrowRight,
  Loader2,
  Settings,
  Undo2,
  User,
} from 'lucide-react';

import { Button } from '@/components/ui/button';
import { Form } from '@/components/ui/form';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import { containerVariants, itemVariants } from '@/lib/animation';
import {
  extractMetadata,
  listEngines,
  listTemplates,
} from '@/server/functions';
import { useStreamerForm, StreamerPayload } from '@/hooks/use-streamer-form';
import { getStreamer } from '@/server/functions';

import { StreamerIdentityFields } from './config/streamer-identity-fields';
import { StreamerGeneralSettings } from './config/streamer-general-settings';
import { StreamerConfiguration } from './config/streamer-configuration';
import { StreamerTabs, StreamerTab } from './config/streamer-tabs';
import { SaveFab } from '@/components/shared/save-fab';

type Streamer = NonNullable<Awaited<ReturnType<typeof getStreamer>>>;

interface StreamerEditorProps {
  mode: 'create' | 'edit';
  /** Required in edit mode; seeds and re-seeds the form. */
  streamer?: Streamer;
  onSubmit: (payload: StreamerPayload) => void;
  isSubmitting: boolean;
  /** Rendered above the tabs in edit mode. */
  header?: ReactNode;
  /** Right-hand column in edit mode (downloads, history, sessions). */
  sidebar?: ReactNode;
  /** Appended after General and Advanced, e.g. the Filters tab. */
  extraTabs?: StreamerTab[];
}

/**
 * The streamer form, in both the modes it is used.
 *
 * `create` runs a two-step wizard so a URL is validated before any configuration is asked for;
 * `edit` shows everything at once alongside live status panels. Both drive one
 * [`useStreamerForm`] instance, so validation, autofill and the submit payload cannot diverge.
 */
export function StreamerEditor({
  mode,
  streamer,
  onSubmit,
  isSubmitting,
  header,
  sidebar,
  extraTabs = [],
}: StreamerEditorProps) {
  const { i18n } = useLingui();
  const navigate = useNavigate();
  const [stage, setStage] = useState<1 | 2>(1);
  const [isCheckingUrl, setIsCheckingUrl] = useState(false);

  const {
    form,
    isAutofilling,
    handleAutofillName,
    trimAndValidateUrl,
    toPayload,
    onInvalid,
  } = useStreamerForm({ streamer });

  const { data: templates } = useQuery({
    queryKey: ['templates'],
    queryFn: () => listTemplates(),
    initialData: [],
  });
  const { data: engines } = useQuery({
    queryKey: ['engines'],
    queryFn: () => listEngines(),
  });

  const isDirty = form.formState.isDirty;

  // Guard against losing edits to an accidental navigation. Submitting resets `isDirty` via the
  // mutation's refetch, so a successful save does not trip this.
  useBlocker({
    shouldBlockFn: () => {
      if (!isDirty || isSubmitting) return false;
      return !window.confirm(
        i18n._(msg`You have unsaved changes. Leave without saving?`),
      );
    },
    enableBeforeUnload: () => isDirty && !isSubmitting,
  });

  const submit = form.handleSubmit(
    (data) => onSubmit(toPayload(data)),
    onInvalid,
  );

  /** Validate the URL against the backend before asking for any configuration. */
  const handleNext = async () => {
    const url = await trimAndValidateUrl();
    if (!url) return;
    if (!(await form.trigger('name'))) return;

    setIsCheckingUrl(true);
    try {
      const metadata = await extractMetadata({ data: url });
      // The backend derives the platform from the URL and rejects a create it cannot resolve, so
      // stop here rather than letting the user fill in step 2 and fail on submit.
      if (!metadata.platform) {
        toast.error(
          i18n._(
            msg`No platform recognizes this link, so it cannot be recorded.`,
          ),
        );
        return;
      }
      setStage(2);
    } catch (error: any) {
      console.error('Failed to extract metadata:', error);
      toast.error(i18n._(msg`Failed to check this link. Please try again.`));
    } finally {
      setIsCheckingUrl(false);
    }
  };

  const tabs: StreamerTab[] = [
    {
      value: 'general',
      label: <Trans>General</Trans>,
      icon: <Settings className="h-4 w-4" />,
      content: (
        <TabCard
          title={<Trans>General Configuration</Trans>}
          description={<Trans>Basic settings for the streamer.</Trans>}
        >
          <StreamerGeneralSettings
            form={form}
            templates={templates}
            onAutofillName={handleAutofillName}
            isAutofilling={isAutofilling}
            // Collected in step 1 of the wizard.
            hideIdentityFields={mode === 'create'}
          />
        </TabCard>
      ),
    },
    {
      value: 'advanced',
      label: <Trans>Advanced</Trans>,
      icon: <Activity className="h-4 w-4" />,
      content: (
        <TabCard
          title={<Trans>Advanced Configuration</Trans>}
          description={
            <Trans>Override global defaults for this streamer.</Trans>
          }
        >
          <StreamerConfiguration
            form={form}
            engines={engines}
            streamerId={streamer?.id}
          />
        </TabCard>
      ),
    },
    ...extraTabs,
  ];

  // ── Create, step 1: identify the stream ──
  if (mode === 'create' && stage === 1) {
    return (
      <Form {...form}>
        <motion.div
          variants={containerVariants}
          initial="hidden"
          animate="visible"
          className="mx-auto max-w-xl p-4 md:p-8"
        >
          <motion.div variants={itemVariants}>
            {/* `py-0` + `overflow-hidden` so the header's tint runs to the card's rounded top
                edge instead of floating below the Card's own vertical padding. */}
            <Card className="gap-0 overflow-hidden border-border/40 bg-card/80 py-0 shadow-sm backdrop-blur-sm">
              <CardHeader className="border-b border-border/40 bg-muted/10 py-5">
                <div className="flex items-center gap-4">
                  <div className="flex flex-col gap-0.5">
                    <CardTitle className="text-lg font-semibold tracking-tight">
                      <Trans>Add a streamer</Trans>
                    </CardTitle>
                    <CardDescription className="text-xs font-normal text-muted-foreground/80">
                      <Trans>
                        Paste a channel link. Everything else can wait.
                      </Trans>
                    </CardDescription>
                  </div>
                  <div className="ml-auto rounded-xl border border-border/50 bg-background/50 p-2 text-primary shadow-sm">
                    <User className="h-5 w-5" />
                  </div>
                </div>
              </CardHeader>

              <CardContent className="space-y-6 p-6">
                <StreamerIdentityFields
                  form={form}
                  onAutofillName={handleAutofillName}
                  isAutofilling={isAutofilling}
                />

                <div className="flex justify-end gap-2 pt-2">
                  <Button
                    type="button"
                    variant="ghost"
                    onClick={() => navigate({ to: '/streamers' })}
                  >
                    <Undo2 className="mr-2 h-4 w-4" />
                    <Trans>Cancel</Trans>
                  </Button>
                  <Button
                    type="button"
                    onClick={handleNext}
                    disabled={isCheckingUrl}
                    className="min-w-32"
                  >
                    {isCheckingUrl ? (
                      <>
                        <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                        <Trans>Checking link</Trans>
                      </>
                    ) : (
                      <>
                        <Trans>Continue</Trans>
                        <ArrowRight className="ml-2 h-4 w-4" />
                      </>
                    )}
                  </Button>
                </div>
              </CardContent>
            </Card>
          </motion.div>
        </motion.div>
      </Form>
    );
  }

  // ── Create step 2, and the whole of edit ──
  return (
    <Form {...form}>
      <motion.div
        variants={containerVariants}
        initial="hidden"
        animate="visible"
        className="mx-auto max-w-7xl space-y-8 p-4 pb-32 md:p-8 md:pb-32"
      >
        {mode === 'create' ? (
          <motion.div variants={itemVariants}>
            <Button type="button" variant="ghost" onClick={() => setStage(1)}>
              <ArrowLeft className="mr-2 h-4 w-4" />
              <Trans>Back to link</Trans>
            </Button>
          </motion.div>
        ) : (
          header && <motion.div variants={itemVariants}>{header}</motion.div>
        )}

        <div className="grid grid-cols-1 gap-8 lg:grid-cols-4">
          <div className={sidebar ? 'lg:col-span-3' : 'lg:col-span-4'}>
            <form id="streamer-form" onSubmit={submit}>
              <motion.div variants={itemVariants}>
                <StreamerTabs tabs={tabs} />
              </motion.div>
            </form>
          </div>

          {sidebar && (
            <motion.div
              variants={itemVariants}
              className="space-y-6 lg:col-span-1"
            >
              {sidebar}
            </motion.div>
          )}
        </div>

        <SaveFab
          isSaving={isSubmitting}
          formId="streamer-form"
          // Nothing is dirty yet on a fresh create, but the action still has to be reachable.
          alwaysVisible={mode === 'create'}
          label={
            mode === 'create' ? (
              <Trans>Create streamer</Trans>
            ) : (
              <Trans>Save changes</Trans>
            )
          }
        />
      </motion.div>
    </Form>
  );
}

/**
 * Panel shell for the editor's tabs, so General and Advanced share one surface treatment rather
 * than each declaring its own header markup.
 *
 * The Filters tab is not wrapped in this: `StreamerFiltersTab` carries its own header row with
 * the "add filter" action in it.
 */
function TabCard({
  title,
  description,
  children,
}: {
  title: ReactNode;
  description: ReactNode;
  children: ReactNode;
}) {
  return (
    <Card className="border-border/40 bg-card/80 shadow-sm backdrop-blur-sm">
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent>{children}</CardContent>
    </Card>
  );
}
