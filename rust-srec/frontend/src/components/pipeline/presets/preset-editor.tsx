import { z } from 'zod';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { JobPresetSchema } from '@/api/schemas';
import { Form } from '@/components/ui/form';
import { t } from '@lingui/core/macro';
import { useEffect } from 'react';
import { toast } from 'sonner';
import { PresetMetaForm } from './editor/preset-meta-form';
import { PresetConfigForm } from './editor/preset-config-form';
import { PresetSaveFab } from './editor/preset-save-fab';
import { getProcessorDefinition } from './processors/registry';

const PresetFormSchema = z.object({
  id: z.string().min(1, t`ID is required`),
  name: z.string().min(1, t`Name is required`),
  description: z.string().optional(),
  category: z.string().optional(),
  processor: z.string().min(1, t`Processor is required`),
  config: z.any(),
});

type PresetFormValues = z.infer<typeof PresetFormSchema>;

interface PresetEditorProps {
  initialData?: z.infer<typeof JobPresetSchema> | null;
  isUpdating?: boolean;
  onSubmit: (data: PresetFormValues) => void;
  title: React.ReactNode;
}

export function PresetEditor({
  initialData,
  isUpdating = false,
  onSubmit,
  title,
}: PresetEditorProps) {
  const form = useForm<PresetFormValues>({
    resolver: zodResolver(PresetFormSchema),
    defaultValues: {
      id: '',
      name: '',
      description: '',
      category: '',
      processor: 'remux',
      config: {},
    },
  });
  console.log('initialData', initialData);

  useEffect(() => {
    if (initialData) {
      form.reset({
        id: initialData.id,
        name: initialData.name,
        description: initialData.description || '',
        category: initialData.category || '',
        processor: initialData.processor,
        config: initialData.config,
      });
    }
  }, [initialData, form]);

  const currentProcessor = form.watch('processor');
  const { isDirty } = form.formState;

  const handleSave = form.handleSubmit((values) => {
    form.clearErrors('config');

    const definition = getProcessorDefinition(values.processor);
    if (definition) {
      const parsed = definition.schema.safeParse(values.config ?? {});
      if (!parsed.success) {
        toast.error(t`Fix configuration errors before saving`);

        for (const issue of parsed.error.issues) {
          const fieldPath = issue.path.length
            ? (`config.${issue.path.join('.')}` as const)
            : ('config' as const);
          form.setError(fieldPath as any, {
            type: 'manual',
            message: issue.message,
          });
        }

        return;
      }

      onSubmit({ ...values, config: parsed.data });
      return;
    }

    onSubmit(values);
  });

  return (
    <div className="min-h-screen pb-20 pt-4">
      <div className="max-w-6xl mx-auto p-4 md:p-6 relative">
        <Form {...form}>
          <form className="grid grid-cols-1 lg:grid-cols-12 gap-6 lg:gap-8 items-start">
            {/* Left Column: Meta Info */}
            <div className="lg:col-span-4 space-y-6">
              <PresetMetaForm
                form={form}
                initialData={initialData}
                title={title}
                isUpdating={isUpdating}
              />
            </div>

            {/* Right Column: Configuration */}
            <div className="lg:col-span-8 space-y-6">
              <PresetConfigForm
                form={form}
                currentProcessor={currentProcessor}
              />
            </div>
          </form>
        </Form>

        <PresetSaveFab
          isDirty={isDirty}
          isUpdating={isUpdating}
          onSave={handleSave}
        />
      </div>
    </div>
  );
}
