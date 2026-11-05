import { Trans } from '@lingui/react/macro';
import { FlagFormField } from '@/components/ui/flag-form-field';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import {
  ProcessorConfigFormProps,
  useProcessorFieldPath,
} from './common-props';
import {
  ProcessorFormShell,
  ProcessorSection,
  ProcessorTextField,
} from './processor-form-layout';
import { MetadataConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { useEffect, useRef, useState } from 'react';
import {
  useFormContext,
  type FieldPath,
  type FieldValues,
} from 'react-hook-form';
import { PlusCircle, Trash2, Tags, Mic2, Settings2 } from 'lucide-react';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';

type MetadataConfig = z.infer<typeof MetadataConfigSchema>;

interface CustomRow {
  id: number;
  key: string;
  value: string;
}

function toRows(
  custom: Record<string, string> | undefined,
  allocateId: () => number,
): CustomRow[] {
  return Object.entries(custom ?? {}).map(([key, value]) => ({
    id: allocateId(),
    key,
    value,
  }));
}

/**
 * Identity of the stored map for change detection.
 *
 * The form hands back a fresh copy on every update, so references say nothing
 * about whether the tags actually changed.
 */
function identityOf(custom: Record<string, string> | undefined): string {
  return JSON.stringify(custom ?? {});
}

/**
 * Editor for the `custom` tag map.
 *
 * The rows are held locally because the stored value is a record: two rows
 * awaiting their names would share the empty key and collapse into one, and a
 * rename mid-typing would re-key the record on every keystroke. Only rows that
 * have a name reach the form, so a half-filled row is simply not saved.
 */
function CustomMetadataFields({ path }: { path: FieldPath<FieldValues> }) {
  const { watch, setValue } = useFormContext();
  const { i18n } = useLingui();
  const stored = watch(path) as Record<string, string> | undefined;
  const nextId = useRef(0);
  const allocateId = () => nextId.current++;

  const [rows, setRows] = useState<CustomRow[]>(() =>
    toRows(stored, allocateId),
  );
  // Tracks the record this editor wrote, so a config load or a form reset
  // rebuilds the rows while our own writes leave them untouched.
  const lastWritten = useRef(identityOf(stored));

  useEffect(() => {
    const identity = identityOf(stored);
    if (identity === lastWritten.current) return;
    lastWritten.current = identity;
    setRows(toRows(stored, () => nextId.current++));
    // `allocateId` is recreated every render; the equivalent inline callback
    // keeps this effect keyed on the stored value alone.
  }, [stored]);

  const writeRows = (next: CustomRow[]) => {
    setRows(next);
    const custom: Record<string, string> = {};
    for (const row of next) {
      if (row.key) custom[row.key] = row.value;
    }
    lastWritten.current = identityOf(custom);
    setValue(path, custom, { shouldDirty: true });
  };

  // Local only: a nameless row has nothing to store yet.
  const addEntry = () => {
    setRows((current) => [
      ...current,
      { id: allocateId(), key: '', value: '' },
    ]);
  };

  const updateRow = (id: number, patch: Partial<CustomRow>) => {
    writeRows(rows.map((row) => (row.id === id ? { ...row, ...patch } : row)));
  };

  return (
    <div className="space-y-3">
      <div className="grid grid-cols-1 gap-2">
        {rows.map((row) => (
          <div key={row.id} className="flex gap-2 items-center group">
            <Input
              placeholder={i18n._(msg`Key`)}
              aria-label={i18n._(msg`Key`)}
              value={row.key}
              onChange={(e) => updateRow(row.id, { key: e.target.value })}
              className="w-1/3 bg-background/50 border-border/50 focus:bg-background h-9 text-sm"
            />
            <Input
              placeholder={i18n._(msg`Value`)}
              aria-label={i18n._(msg`Value`)}
              value={row.value}
              onChange={(e) => updateRow(row.id, { value: e.target.value })}
              className="flex-1 bg-background/50 border-border/50 focus:bg-background h-9 text-sm"
            />
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="h-9 w-9 text-muted-foreground/50 hover:text-destructive hover:bg-destructive/10"
              onClick={() =>
                writeRows(rows.filter((entry) => entry.id !== row.id))
              }
              aria-label={i18n._(msg`Remove custom tag`)}
            >
              <Trash2 className="h-4 w-4" />
            </Button>
          </div>
        ))}
      </div>
      {rows.length === 0 && (
        <div className="text-xs text-muted-foreground text-center py-4 border border-dashed border-border/50 rounded-lg">
          <Trans>No custom tags added</Trans>
        </div>
      )}
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="w-full border-dashed border-border/60 hover:border-primary/50 hover:bg-primary/5 text-muted-foreground hover:text-primary h-9"
        onClick={addEntry}
      >
        <PlusCircle className="mr-2 h-3.5 w-3.5" />
        <Trans>Add Custom Tag</Trans>
      </Button>
    </div>
  );
}

export function MetadataConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps) {
  const { i18n } = useLingui();
  const fieldPath = useProcessorFieldPath<MetadataConfig>(pathPrefix);

  return (
    <ProcessorFormShell>
      <div className="space-y-6">
        {/* Track Info Section */}
        <ProcessorSection
          icon={Mic2}
          iconClassName="text-pink-500"
          title={<Trans>Track Information</Trans>}
        >
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <ProcessorTextField
              control={control}
              name={fieldPath('artist')}
              label={<Trans>Artist</Trans>}
              placeholder={i18n._(msg`Artist`)}
            />
            <ProcessorTextField
              control={control}
              name={fieldPath('title')}
              label={<Trans>Title</Trans>}
              placeholder={i18n._(msg`Title`)}
            />
            <ProcessorTextField
              control={control}
              name={fieldPath('album')}
              label={<Trans>Album</Trans>}
            />
            <ProcessorTextField
              control={control}
              name={fieldPath('date')}
              label={<Trans>Date</Trans>}
              placeholder={i18n._(msg`YYYY-MM-DD`)}
            />
            <div className="md:col-span-2">
              <ProcessorTextField
                control={control}
                name={fieldPath('comment')}
                label={<Trans>Comment</Trans>}
              />
            </div>
          </div>
        </ProcessorSection>

        {/* Custom Fields Section */}
        <ProcessorSection
          icon={Tags}
          iconClassName="text-purple-500"
          title={<Trans>Custom Tags</Trans>}
        >
          <CustomMetadataFields path={fieldPath('custom')} />
        </ProcessorSection>

        {/* Options Section */}
        <ProcessorSection
          icon={Settings2}
          iconClassName="text-gray-500"
          title={<Trans>File Operations</Trans>}
        >
          <FlagFormField
            fieldName={fieldPath('overwrite')}
            title={<Trans>Overwrite</Trans>}
            description={<Trans>Overwrite existing files</Trans>}
          />
          <FlagFormField
            fieldName={fieldPath('remove_input_on_success')}
            title={<Trans>Remove source on success</Trans>}
            description={
              <Trans>Delete source file after successful processing</Trans>
            }
          />
        </ProcessorSection>
      </div>
    </ProcessorFormShell>
  );
}
