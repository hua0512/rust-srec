import { Trans } from '@lingui/react/macro';
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
import { Button } from '@/components/ui/button';
import { ProcessorConfigFormProps } from './common-props';
import { MetadataConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { useEffect, useRef, useState } from 'react';
import { useFormContext } from 'react-hook-form';
import { PlusCircle, Trash2, Tags, Mic2, Settings2 } from 'lucide-react';
import { motion } from 'motion/react';
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
function CustomMetadataFields({ basePath }: { basePath: string }) {
  const { watch, setValue } = useFormContext();
  const { i18n } = useLingui();
  const path = (basePath ? `${basePath}.custom` : 'custom') as any;

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
              value={row.key}
              onChange={(e) => updateRow(row.id, { key: e.target.value })}
              className="w-1/3 bg-background/50 border-border/50 focus:bg-background h-9 text-sm"
            />
            <Input
              placeholder={i18n._(msg`Value`)}
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
}: ProcessorConfigFormProps<MetadataConfig>) {
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
      <div className="space-y-6">
        {/* Track Info Section */}
        <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
          <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
            <Mic2 className="w-4 h-4 text-pink-500" />
            <h3 className="font-semibold text-sm mr-auto">
              <Trans>Track Information</Trans>
            </h3>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FormField
              control={control}
              name={`${prefix}artist` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Artist</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg"
                      placeholder={i18n._(msg`Artist`)}
                      {...field}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
            <FormField
              control={control}
              name={`${prefix}title` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Title</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg"
                      placeholder={i18n._(msg`Title`)}
                      {...field}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
            <FormField
              control={control}
              name={`${prefix}album` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Album</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg"
                      {...field}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
            <FormField
              control={control}
              name={`${prefix}date` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Date</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg"
                      placeholder={i18n._(msg`YYYY-MM-DD`)}
                      {...field}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={`${prefix}comment` as any}
              render={({ field }) => (
                <FormItem className="col-span-1 md:col-span-2">
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Comment</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg"
                      {...field}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>
        </div>

        {/* Custom Fields Section */}
        <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
          <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
            <Tags className="w-4 h-4 text-purple-500" />
            <h3 className="font-semibold text-sm mr-auto">
              <Trans>Custom Tags</Trans>
            </h3>
          </div>
          <CustomMetadataFields basePath={pathPrefix || ''} />
        </div>

        {/* Options Section */}
        <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4">
          <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
            <Settings2 className="w-4 h-4 text-gray-500" />
            <h3 className="font-semibold text-sm mr-auto">
              <Trans>File Operations</Trans>
            </h3>
          </div>

          <FormField
            control={control}
            name={`${prefix}overwrite` as any}
            render={({ field }) => (
              <FormItem className="flex flex-row items-center justify-between rounded-xl border border-border/40 p-4 shadow-sm bg-muted/10 transition-colors hover:bg-muted/20">
                <div className="space-y-1">
                  <FormLabel className="text-sm font-medium">
                    <Trans>Overwrite</Trans>
                  </FormLabel>
                  <FormDescription className="text-xs">
                    <Trans>Overwrite existing files</Trans>
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
                    <Trans>Remove source on success</Trans>
                  </FormLabel>
                  <FormDescription className="text-xs">
                    <Trans>
                      Delete source file after successful processing
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
    </motion.div>
  );
}
