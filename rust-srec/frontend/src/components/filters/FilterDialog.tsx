import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Filter as FilterIcon } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Form } from '@/components/ui/form';
import { type SubmitHandler, useForm } from 'react-hook-form';
import { zodResolver } from '@hookform/resolvers/zod';
import { z } from 'zod';
import {
  CreateFilterRequestSchema,
  FilterSchema,
  type FilterType,
  normalizeFilterConfigForType,
} from '../../api/schemas';
import { createFilter, updateFilter } from '@/server/functions';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { toast } from 'sonner';
import { FilterTypeSelector } from './forms/FilterTypeSelector';
import { TimeBasedFilterForm } from './forms/TimeBasedFilterForm';
import { KeywordFilterForm } from './forms/KeywordFilterForm';
// import { CategoryFilterForm } from './forms/CategoryFilterForm';
import { CronFilterForm } from './forms/CronFilterForm';
import { RegexFilterForm } from './forms/RegexFilterForm';
import { useEffect } from 'react';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';

// Union of all possible configs for the form state
const FormSchema = CreateFilterRequestSchema;
type FormInput = z.input<typeof FormSchema>;
type FormOutput = z.infer<typeof FormSchema>;

type Filter = z.infer<typeof FilterSchema>;

interface FilterDialogProps {
  streamerId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  filterToEdit?: Filter | null;
}

export function FilterDialog({
  streamerId,
  open,
  onOpenChange,
  filterToEdit,
}: FilterDialogProps) {
  const { i18n } = useLingui();
  const queryClient = useQueryClient();
  const isEditing = !!filterToEdit;

  const form = useForm<FormInput, any, FormOutput>({
    resolver: zodResolver(FormSchema),
    defaultValues: {
      filter_type: 'KEYWORD',
      config: { include: [], exclude: [] },
    },
  });

  // Reset/Populate form when opening/editing
  useEffect(() => {
    if (open) {
      if (filterToEdit) {
        // We need to parse the config if it's coming from the API (it's already parsed object though)
        // But we need to ensure it matches the schema expected by the form for that type.
        // The API returns config as `any` (or `Value`), so we pass it directly.
        // We need to valid cast filter_type string to enum.
        const filterType = filterToEdit.filter_type as FilterType;
        const normalizedConfig = normalizeFilterConfigForType(
          filterType,
          filterToEdit.config,
        );
        form.reset({
          filter_type: filterType,
          config: normalizedConfig as any,
        });
      } else {
        form.reset({
          filter_type: 'KEYWORD',
          config: { include: [], exclude: [] },
        });
      }
    }
  }, [open, filterToEdit, form]);

  // Watch filter type to switch sub-forms
  const filterType = form.watch('filter_type');

  // Reset config when filter type changes (if not initial edit load)
  // Actually, handling this cleanly is tricky.
  // If user switches type, the config structure becomes invalid for the new type.
  // We should probably set default config for the new type.
  useEffect(() => {
    const subscription = form.watch((value, { name }) => {
      if (name === 'filter_type') {
        const type = value.filter_type;
        let defaultConfig: any = {};
        switch (type) {
          case 'TIME_BASED':
            defaultConfig = {
              days_of_week: [],
              start_time: '00:00:00',
              end_time: '23:59:59',
            };
            break;
          case 'KEYWORD':
            defaultConfig = {
              include: [],
              exclude: [],
            };
            break;
          /*
          case 'CATEGORY':
            defaultConfig = { categories: [], exclude: false };
            break;
          */
          case 'CRON':
            defaultConfig = { expression: '* * * * * *', timezone: 'UTC' };
            break;
          case 'REGEX':
            defaultConfig = {
              pattern: '',
              exclude: false,
              case_insensitive: false,
            };
            break;
        }
        form.setValue('config', defaultConfig as any);
        form.clearErrors('config');
      }
    });
    return () => subscription.unsubscribe();
  }, [form]);

  const createMutation = useMutation({
    mutationFn: (data: z.infer<typeof CreateFilterRequestSchema>) =>
      createFilter({ data: { streamerId, data } }),
    onSuccess: () => {
      toast.success(i18n._(msg`Filter created successfully`));
      void queryClient.invalidateQueries({
        queryKey: ['streamers', streamerId, 'filters'],
      });
      onOpenChange(false);
    },
    onError: (error: any) => {
      toast.error(error.message || i18n._(msg`Failed to create filter`));
    },
  });

  const updateMutation = useMutation({
    mutationFn: (data: z.infer<typeof CreateFilterRequestSchema>) =>
      updateFilter({ data: { streamerId, filterId: filterToEdit!.id, data } }),
    onSuccess: () => {
      toast.success(i18n._(msg`Filter updated successfully`));
      void queryClient.invalidateQueries({
        queryKey: ['streamers', streamerId, 'filters'],
      });
      onOpenChange(false);
    },
    onError: (error: any) => {
      toast.error(error.message || i18n._(msg`Failed to update filter`));
    },
  });

  const onSubmit: SubmitHandler<FormOutput> = (data) => {
    if (isEditing) {
      updateMutation.mutate(data);
    } else {
      createMutation.mutate(data);
    }
  };

  const renderSubForm = () => {
    switch (filterType) {
      case 'TIME_BASED':
        return <TimeBasedFilterForm />;
      case 'KEYWORD':
        return <KeywordFilterForm />;
      /*
      case 'CATEGORY':
        return <CategoryFilterForm />;
      */
      case 'CRON':
        return <CronFilterForm />;
      case 'REGEX':
        return <RegexFilterForm />;
      default:
        return null;
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl gap-0 p-0 overflow-hidden border-none shadow-2xl flex flex-col max-h-[85vh]">
        <DialogHeader className="px-6 py-6 bg-muted/30 border-b flex-shrink-0">
          <div className="flex items-center gap-3">
            <div className="p-2 bg-primary/10 rounded-lg">
              <FilterIcon className="w-5 h-5 text-primary" />
            </div>
            <div className="space-y-1 text-left">
              <DialogTitle>
                {isEditing ? (
                  <Trans>Edit Filter</Trans>
                ) : (
                  <Trans>Add New Filter</Trans>
                )}
              </DialogTitle>
              <DialogDescription>
                <Trans>Configure recording rules for this streamer.</Trans>
              </DialogDescription>
            </div>
          </div>
        </DialogHeader>

        <div className="flex-1 overflow-y-auto min-h-0">
          <Form {...form}>
            <form
              id="filter-form"
              onSubmit={form.handleSubmit(onSubmit)}
              className="space-y-6 px-6 py-6"
            >
              <FilterTypeSelector />

              <div className="rounded-xl border bg-card text-card-foreground shadow-sm p-5 transition-all">
                {renderSubForm()}
              </div>

              {/* Hidden submit button to ensure Enter key submission works reliably within dialog */}
              <button type="submit" className="hidden" aria-hidden="true" />
            </form>
          </Form>
        </div>

        <DialogFooter className="px-6 py-4 bg-muted/30 border-t flex-shrink-0 gap-2">
          <Button
            type="button"
            variant="outline"
            onClick={() => onOpenChange(false)}
          >
            <Trans>Cancel</Trans>
          </Button>
          <Button
            type="submit"
            form="filter-form"
            disabled={createMutation.isPending || updateMutation.isPending}
            className="min-w-[120px]"
          >
            {createMutation.isPending || updateMutation.isPending ? (
              <span className="animate-pulse">
                <Trans>Saving...</Trans>
              </span>
            ) : isEditing ? (
              <Trans>Save Changes</Trans>
            ) : (
              <Trans>Create Filter</Trans>
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
