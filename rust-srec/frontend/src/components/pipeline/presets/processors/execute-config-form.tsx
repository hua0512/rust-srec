import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import {
  FormField,
  FormItem,
  FormLabel,
  FormControl,
  FormMessage,
  FormDescription,
} from '@/components/ui/form';
import { Textarea } from '@/components/ui/textarea';
import { ConfigFieldLabel } from '@/components/config/shared/config-field';
import {
  ProcessorConfigFormProps,
  useProcessorFieldPath,
} from './common-props';
import {
  ProcessorFormShell,
  ProcessorSection,
  ProcessorTextField,
} from './processor-form-layout';
import { ExecuteConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { Badge } from '@/components/ui/badge';
import { Terminal, FolderSearch, Plus, Trash2 } from 'lucide-react';
import { useId } from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import { Button } from '@/components/ui/button';
import { Label } from '@/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';

type ExecuteConfig = z.infer<typeof ExecuteConfigSchema>;

export function ExecuteConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps) {
  const { i18n } = useLingui();
  const fieldPath = useProcessorFieldPath<ExecuteConfig>(pathPrefix);
  const { unregister, setValue, getValues, clearErrors } = useFormContext();
  const modeId = useId();
  const program = useWatch({ control, name: fieldPath('program') });
  const mode = program == null ? 'command' : 'program';

  const changeMode = (nextMode: string) => {
    // Radix can emit an empty value while a surrounding form is reset.
    if ((nextMode !== 'command' && nextMode !== 'program') || nextMode === mode)
      return;
    // Unregister also removes defaults, so a hidden mode cannot reappear on save.
    unregister([fieldPath('command'), fieldPath('program'), fieldPath('args')]);
    setValue(fieldPath(nextMode), '', { shouldDirty: true });
    if (nextMode === 'program') {
      setValue(fieldPath('args'), [], { shouldDirty: true });
    }
  };

  return (
    <ProcessorFormShell>
      <div className="space-y-6">
        <ProcessorSection
          icon={Terminal}
          iconClassName="text-green-500"
          title={<Trans>Execution</Trans>}
        >
          <div className="grid grid-cols-1 gap-6">
            <div className="space-y-2">
              <Label htmlFor={modeId}>
                <Trans>Execution mode</Trans>
              </Label>
              <Select value={mode} onValueChange={changeMode}>
                <SelectTrigger id={modeId} className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="command">
                    <Trans>Shell command</Trans>
                  </SelectItem>
                  <SelectItem value="program">
                    <Trans>Program and arguments</Trans>
                  </SelectItem>
                </SelectContent>
              </Select>
              <p className="text-sm text-muted-foreground">
                <Trans>
                  Switching modes clears the previous command or program and
                  arguments.
                </Trans>
              </p>
            </div>

            {mode === 'program' ? (
              <>
                <ProcessorTextField
                  control={control}
                  name={fieldPath('program')}
                  label={<Trans>Program</Trans>}
                  description={
                    <Trans>
                      Fixed executable name or path on the server. Placeholders
                      are expanded only in arguments. On Windows servers, use
                      shell command mode for .bat or .cmd files.
                    </Trans>
                  }
                  placeholder="ffmpeg"
                  className="font-mono"
                />
                <FormField
                  control={control}
                  name={fieldPath('args')}
                  render={({ field }) => {
                    const args: unknown[] = Array.isArray(field.value)
                      ? field.value
                      : [];
                    const updateArgs = (
                      update: (current: unknown[]) => unknown[],
                    ) => {
                      // Child Controllers update individual entries without
                      // rerendering this array Controller. Read the latest argv
                      // before adding/removing so recent edits are retained.
                      const current = getValues(fieldPath('args'));
                      clearErrors(fieldPath('args'));
                      field.onChange(
                        update(Array.isArray(current) ? current : []),
                      );
                    };
                    return (
                      <FormItem>
                        <div className="flex items-center justify-between gap-2">
                          <span className="text-sm font-medium">
                            <Trans>Arguments</Trans>
                          </span>
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            onClick={() =>
                              updateArgs((current) => [...current, ''])
                            }
                          >
                            <Plus className="size-4" />
                            <Trans>Add argument</Trans>
                          </Button>
                        </div>
                        <FormDescription>
                          <Trans>
                            Each box is one argument, in order. Empty arguments,
                            spaces, quotes and line breaks are preserved. Do not
                            add shell quotes. Placeholders below are expanded
                            without shell interpretation.
                          </Trans>
                        </FormDescription>
                        {args.map((_, index) => {
                          const number = index + 1;
                          return (
                            <FormField
                              key={index}
                              control={control}
                              name={fieldPath(`args.${index}`)}
                              render={({ field: argument }) => (
                                <FormItem>
                                  <div className="flex items-center justify-between gap-2">
                                    <FormLabel>
                                      <Trans>Argument {number}</Trans>
                                    </FormLabel>
                                    <Button
                                      type="button"
                                      variant="ghost"
                                      size="icon"
                                      aria-label={i18n._(
                                        msg`Remove argument ${number}`,
                                      )}
                                      onClick={() =>
                                        updateArgs((current) =>
                                          current.filter(
                                            (_, itemIndex) =>
                                              itemIndex !== index,
                                          ),
                                        )
                                      }
                                    >
                                      <Trash2 className="size-4" />
                                    </Button>
                                  </div>
                                  <FormControl>
                                    <Textarea
                                      rows={2}
                                      className="font-mono"
                                      {...argument}
                                      value={argument.value ?? ''}
                                    />
                                  </FormControl>
                                  <FormMessage />
                                </FormItem>
                              )}
                            />
                          );
                        })}
                        <FormMessage />
                      </FormItem>
                    );
                  }}
                />
              </>
            ) : (
              <FormField
                control={control}
                name={fieldPath('command')}
                render={({ field }) => (
                  <FormItem>
                    <ConfigFieldLabel>
                      <Trans>Command</Trans>
                    </ConfigFieldLabel>
                    <FormControl>
                      <Textarea
                        placeholder="ffmpeg -nostdin -n -i {input} -c copy {output}"
                        className="font-mono bg-background/50 border-border/50 focus:bg-background rounded-lg text-sm"
                        rows={5}
                        {...field}
                        value={field.value ?? ''}
                      />
                    </FormControl>
                    <FormDescription className="mt-2 text-sm">
                      <Trans>
                        Runs in the server's shell. With placeholders, use a
                        fixed command name and ordinary arguments or file
                        redirects. Shell expansions, command groups and nested
                        shells are unsupported. Windows servers also reject
                        pipes, multiline templates, batch scripts and builtins
                        with placeholders, such as echo.
                      </Trans>
                    </FormDescription>
                    <FormMessage />
                  </FormItem>
                )}
              />
            )}
            {/* Keep the block legend outside FormDescription's paragraph for SSR. */}
            <div className="mt-2 text-sm max-w-full text-muted-foreground">
              <div className="p-3 border border-border/40 rounded-lg bg-muted/20">
                <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
                  <div className="space-y-2">
                    <div className="mb-2 font-semibold text-[10px] uppercase tracking-wide opacity-70">
                      <Trans>Path Variables</Trans>
                    </div>
                    <div className="flex flex-wrap gap-2">
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-background/50 cursor-help border-border/50"
                        title={i18n._(msg`First input file path`)}
                      >
                        {'{input}'}
                      </Badge>
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-background/50 cursor-help border-border/50"
                        title={i18n._(msg`First output file path`)}
                      >
                        {'{output}'}
                      </Badge>
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-background/50 cursor-help border-border/50"
                        title={i18n._(msg`JSON array of all inputs`)}
                      >
                        {'{inputs_json}'}
                      </Badge>
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-background/50 cursor-help border-border/50"
                        title={i18n._(msg`JSON array of all outputs`)}
                      >
                        {'{outputs_json}'}
                      </Badge>
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-background/50 cursor-help border-border/50"
                        title={i18n._(
                          msg`JSON of the session pairing (video and danmu per segment), or null`,
                        )}
                      >
                        {'{manifest_json}'}
                      </Badge>
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-background/50 cursor-help border-border/50"
                        title={i18n._(msg`Nth input: {input0}, {input1}...`)}
                      >
                        {'{inputN}'}
                      </Badge>
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-background/50 cursor-help border-border/50"
                        title={i18n._(msg`Nth output: {output0}, {output1}...`)}
                      >
                        {'{outputN}'}
                      </Badge>
                    </div>
                  </div>

                  <div className="space-y-2">
                    <div className="mb-2 font-semibold text-[10px] uppercase tracking-wide opacity-70">
                      <Trans>Metadata Variables</Trans>
                    </div>
                    <div className="flex flex-wrap gap-2">
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-background/50 cursor-help border-border/50"
                        title={i18n._(msg`Streamer ID`)}
                      >
                        {'{streamer_id}'}
                      </Badge>
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-background/50 cursor-help border-border/50"
                        title={i18n._(msg`Session ID`)}
                      >
                        {'{session_id}'}
                      </Badge>
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-background/50 cursor-help border-border/50"
                        title={i18n._(msg`Sanitized streamer name`)}
                      >
                        {'{streamer}'}
                      </Badge>
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-background/50 cursor-help border-border/50"
                        title={i18n._(msg`Sanitized session title`)}
                      >
                        {'{title}'}
                      </Badge>
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-background/50 cursor-help border-border/50"
                        title={i18n._(msg`Platform name`)}
                      >
                        {'{platform}'}
                      </Badge>
                    </div>
                  </div>

                  <div className="col-span-full space-y-2 border-t border-border/20 pt-2 mt-2">
                    <div className="mb-2 font-semibold text-[10px] uppercase tracking-wide opacity-70">
                      <Trans>Time Placeholders (Local Time)</Trans>
                    </div>
                    <div className="flex flex-wrap gap-2">
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-muted/30 cursor-help border-dashed"
                        title={i18n._(msg`Year (4 digits)`)}
                      >
                        %Y
                      </Badge>
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-muted/30 cursor-help border-dashed"
                        title={i18n._(msg`Month (01-12)`)}
                      >
                        %m
                      </Badge>
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-muted/30 cursor-help border-dashed"
                        title={i18n._(msg`Day (01-31)`)}
                      >
                        %d
                      </Badge>
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-muted/30 cursor-help border-dashed"
                        title={i18n._(msg`Hour (00-23)`)}
                      >
                        %H
                      </Badge>
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-muted/30 cursor-help border-dashed"
                        title={i18n._(msg`Minute (00-59)`)}
                      >
                        %M
                      </Badge>
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-muted/30 cursor-help border-dashed"
                        title={i18n._(msg`Second (00-59)`)}
                      >
                        %S
                      </Badge>
                      <Badge
                        variant="outline"
                        className="font-mono text-[10px] bg-muted/30 cursor-help border-dashed"
                        title={i18n._(msg`Unix timestamp`)}
                      >
                        %t
                      </Badge>
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </ProcessorSection>

        {/* Output Scanning Section */}
        <ProcessorSection
          icon={FolderSearch}
          iconClassName="text-blue-500"
          title={<Trans>Output Detection</Trans>}
          actions={
            <Badge variant="outline" className="text-[10px]">
              <Trans>Optional</Trans>
            </Badge>
          }
        >
          <FormDescription className="text-xs text-muted-foreground">
            <Trans>
              Scan a directory for new files created by the command. Detected
              files will be passed to the next pipeline step.
            </Trans>
          </FormDescription>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <ProcessorTextField
              control={control}
              name={fieldPath('scan_output_dir')}
              label={<Trans>Scan Directory</Trans>}
              description={
                <Trans>
                  Directory to scan for new files after command execution
                </Trans>
              }
              placeholder={i18n._(msg`e.g. /output/processed/`)}
              className="font-mono text-sm"
            />
            <ProcessorTextField
              control={control}
              name={fieldPath('scan_extension')}
              label={<Trans>File Extension Filter</Trans>}
              description={
                <Trans>
                  Only include files with this extension (without dot)
                </Trans>
              }
              placeholder={i18n._(msg`e.g. mp4`)}
              className="font-mono text-sm"
            />
          </div>
        </ProcessorSection>
      </div>
    </ProcessorFormShell>
  );
}
