import { z } from 'zod';
import { useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { JobPresetSchema } from '@/api/schemas';
import { Form } from '@/components/ui/form';
import { t } from '@lingui/core/macro';
import { useEffect } from 'react';
import { PresetMetaForm } from './editor/preset-meta-form';
import { PresetConfigForm } from './editor/preset-config-form';
import { PresetSaveFab } from './editor/preset-save-fab';

const PresetFormSchema = z.object({
  id: z.string().min(1, t`ID is required`),
  name: z.string().min(1, t`Name is required`),
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
      processor: 'remux',
      config: {},
    },
  });

  useEffect(() => {
    if (initialData) {
      let parsedConfig: any = initialData.config;
      try {
        if (typeof initialData.config === 'string') {
          parsedConfig = JSON.parse(initialData.config);
        }
      } catch {
        parsedConfig = {};
      }

      form.reset({
        id: initialData.id,
        name: initialData.name,
        processor: initialData.processor,
        config: parsedConfig,
      });
    }
  }, [initialData, form]);

  const currentProcessor = form.watch('processor');
  const { isDirty } = form.formState;

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
          onSave={form.handleSubmit(onSubmit)}
        />
      </div>
    </div>
  );
}
