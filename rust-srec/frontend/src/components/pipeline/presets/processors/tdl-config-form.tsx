import { useLingui } from '@lingui/react';
import { t } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { memo, useState } from 'react';
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
import { ListInput } from '@/components/ui/list-input';
import { ProcessorConfigFormProps } from './common-props';
import { TdlProcessorConfigSchema } from '@/api/schemas/tdl';
import { z } from 'zod';
import { motion } from 'motion/react';
import { Badge } from '@/components/ui/badge';
import {
    Terminal,
    Settings,
    Upload,
    Lock,
    ShieldCheck,
    AlertTriangle,
    RefreshCw,
    PlusCircle,
    Trash2,
} from 'lucide-react';
import { TdlLoginDialog } from './tdl-login-dialog';
import { Button } from '@/components/ui/button';
import { useWatch, useFormContext } from 'react-hook-form';
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from '@/components/ui/select';

type TdlConfig = z.infer<typeof TdlProcessorConfigSchema>;

// Helper component for key-value pairs (Environment Variables)
function EnvVarsFields({ basePath }: { basePath: string }) {
    const { watch, setValue } = useFormContext();
    const { i18n } = useLingui();

    const env = (watch((basePath ? `${basePath}.env` : 'env') as any) ||
        {}) as Record<string, string>;
    const entries = Object.entries(env);

    const addEntry = () => {
        const newEnv = { ...env, '': '' };
        setValue((basePath ? `${basePath}.env` : 'env') as any, newEnv, {
            shouldDirty: true,
        });
    };

    const removeEntry = (keyToRemove: string) => {
        const newEnv = { ...env };
        delete newEnv[keyToRemove];
        setValue((basePath ? `${basePath}.env` : 'env') as any, newEnv, {
            shouldDirty: true,
        });
    };

    const updateEntryKey = (oldKey: string, newKey: string, value: string) => {
        if (oldKey === newKey) return;
        const newEnv = { ...env };
        delete newEnv[oldKey];
        newEnv[newKey] = value;
        setValue((basePath ? `${basePath}.env` : 'env') as any, newEnv, {
            shouldDirty: true,
        });
    };

    const updateEntryValue = (key: string, newValue: string) => {
        const newEnv = { ...env };
        newEnv[key] = newValue;
        setValue((basePath ? `${basePath}.env` : 'env') as any, newEnv, {
            shouldDirty: true,
        });
    };

    return (
        <div className="space-y-3 pt-2">
            <div className="grid grid-cols-1 gap-2">
                {entries.map(([key, value], index) => (
                    <div key={index} className="flex gap-2 items-center group">
                        <Input
                            placeholder={t(i18n)`Key`}
                            defaultValue={key}
                            onBlur={(e) => updateEntryKey(key, e.target.value, value)}
                            className="w-1/3 bg-background/50 border-border/50 focus:bg-background h-9 text-xs font-mono"
                        />
                        <Input
                            placeholder={t(i18n)`Value`}
                            value={value}
                            onChange={(e) => updateEntryValue(key, e.target.value)}
                            className="flex-1 bg-background/50 border-border/50 focus:bg-background h-9 text-xs font-mono"
                        />
                        <Button
                            type="button"
                            variant="ghost"
                            size="icon"
                            className="h-9 w-9 text-muted-foreground/50 hover:text-destructive hover:bg-destructive/10"
                            onClick={() => removeEntry(key)}
                        >
                            <Trash2 className="h-4 w-4" />
                        </Button>
                    </div>
                ))}
            </div>
            {entries.length === 0 && (
                <div className="text-[10px] text-muted-foreground text-center py-3 border border-dashed border-border/50 rounded-lg">
                    <Trans>No environment variables defined</Trans>
                </div>
            )}
            <Button
                type="button"
                variant="outline"
                size="sm"
                className="w-full border-dashed border-border/60 hover:border-primary/50 hover:bg-primary/5 text-muted-foreground hover:text-primary h-8 text-[10px]"
                onClick={addEntry}
            >
                <PlusCircle className="mr-2 h-3.5 w-3.5" />
                <Trans>Add Environment Variable</Trans>
            </Button>
        </div>
    );
}

export const TdlConfigForm = memo(function TdlConfigForm({
    control,
    pathPrefix,
}: ProcessorConfigFormProps<TdlConfig>) {
    const { i18n } = useLingui();
    const prefix = pathPrefix ? `${pathPrefix}.` : '';
    const [loginDialogOpen, setLoginDialogOpen] = useState(false);

    // Watch tdl_path and working_dir to pass to login dialog
    const tdlPath = useWatch({ control, name: `${prefix}tdl_path` as any });
    const workingDir = useWatch({ control, name: `${prefix}working_dir` as any });
    const env = useWatch({ control, name: `${prefix}env` as any });
    const uploadAll = useWatch({ control, name: `${prefix}upload_all` as any });
    const loginType = useWatch({ control, name: `${prefix}login_type` as any });
    const namespace = useWatch({ control, name: `${prefix}namespace` as any });
    const storage = useWatch({ control, name: `${prefix}storage` as any });
    const telegramDesktopDir = useWatch({
        control,
        name: `${prefix}telegram_desktop_dir` as any,
    });
    const loginArgs = useWatch({ control, name: `${prefix}login_args` as any });

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
        <div className="p-4 rounded-xl bg-primary/5 border border-primary/20 space-y-4 shadow-sm">
          <div className="flex items-center gap-2 pb-2 border-b border-primary/20 mb-2">
            <Lock className="w-4 h-4 text-primary" />
            <h3 className="font-semibold text-sm mr-auto text-primary">
              <Trans>Authentication & Setup</Trans>
            </h3>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setLoginDialogOpen(true)}
              className="h-8 gap-2 rounded-lg border-primary/20 hover:bg-primary/10 transition-colors shadow-sm"
            >
              <ShieldCheck className="w-3.5 h-3.5" />
              <Trans>Login to Telegram</Trans>
            </Button>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FormField
              control={control}
              name={`${prefix}login_type` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Login Type</Trans>
                  </FormLabel>
                  <Select
                    key={field.value ?? 'initial'}
                    onValueChange={field.onChange}
                    defaultValue={field.value ?? 'auto'}
                  >
                    <FormControl>
                      <SelectTrigger className="h-11 bg-background/50 border-border/50">
                        <SelectValue placeholder={t(i18n)`Select login type`} />
                      </SelectTrigger>
                    </FormControl>
                      <SelectContent>
                      <SelectItem value="auto">
                        <Trans>Auto (QR → Phone & Code → Desktop)</Trans>
                      </SelectItem>
                      <SelectItem value="qr">
                        <Trans>QR Code</Trans>
                      </SelectItem>
                      <SelectItem value="code">
                        <Trans>Phone & Code (2FA optional)</Trans>
                      </SelectItem>
                      <SelectItem value="desktop">
                        <Trans>Telegram Desktop</Trans>
                      </SelectItem>
                    </SelectContent>
                  </Select>
                  <FormDescription className="text-[10px] ml-1">
                    <Trans>
                      Prefer QR login first; Phone & Code (2FA optional) second; Desktop last.
                    </Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            <div className="rounded-xl border border-orange-500/20 bg-orange-500/5 p-3 flex items-start gap-2">
              <ShieldCheck className="w-4 h-4 text-orange-600 mt-0.5" />
              <div className="min-w-0">
                <div className="text-xs font-medium text-orange-600">
                  <Trans>Telegram 2FA password is not supported</Trans>
                </div>
                <div className="text-[11px] text-muted-foreground mt-1">
                  <Trans>
                    If your account has Telegram 2FA enabled, use Desktop login (`-T desktop`) or run `tdl login` locally in a terminal.
                  </Trans>
                </div>
              </div>
            </div>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-6 pt-2">
            <FormField
              control={control}
              name={`${prefix}namespace` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Namespace</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm transition-colors"
                      placeholder="default"
                      {...field}
                      value={field.value ?? ''}
                      onChange={(e) => {
                        const v = e.target.value;
                        field.onChange(v.trim().length ? v : undefined);
                      }}
                    />
                  </FormControl>
                  <FormDescription className="text-[10px] ml-1">
                    <Trans>
                      TDL account namespace (`tdl --ns ...`). Use this to manage multiple Telegram accounts.
                    </Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={`${prefix}storage` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Storage</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm transition-colors"
                      placeholder={t(i18n)`type=bolt,path=/data/tdl`}
                      {...field}
                      value={field.value ?? ''}
                      onChange={(e) => {
                        const v = e.target.value;
                        field.onChange(v.trim().length ? v : undefined);
                      }}
                    />
                  </FormControl>
                  <FormDescription className="text-[10px] ml-1">
                    <Trans>
                      Where TDL stores session data (`tdl --storage ...`). Example: `type=bolt,path=/data/tdl`.
                    </Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
            <FormField
              control={control}
              name={`${prefix}tdl_path` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>TDL Binary Path</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm transition-colors"
                      placeholder="tdl"
                      {...field}
                      value={field.value ?? ''}
                      onChange={(e) => {
                        const v = e.target.value;
                        field.onChange(v.trim().length ? v : undefined);
                      }}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={`${prefix}working_dir` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Working Directory</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm transition-colors"
                      placeholder={t(i18n)`e.g. /data/tdl`}
                      {...field}
                      value={field.value ?? ''}
                      onChange={(e) => {
                        const v = e.target.value;
                        field.onChange(v.trim().length ? v : undefined);
                      }}
                    />
                  </FormControl>
                  <FormDescription className="text-[10px] ml-1">
                    <Trans>Where TDL stores its session and configuration.</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-6 pt-2">
            <FormField
              control={control}
              name={`${prefix}telegram_desktop_dir` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Telegram Desktop Directory</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg font-mono text-sm transition-colors"
                      placeholder={t(i18n)`e.g. C:\\Users\\...\\AppData\\Roaming\\Telegram Desktop`}
                      {...field}
                      value={field.value ?? ''}
                      onChange={(e) => {
                        const v = e.target.value;
                        field.onChange(v.trim().length ? v : undefined);
                      }}
                    />
                  </FormControl>
                  <FormDescription className="text-[10px] ml-1">
                    <Trans>
                      Used for Desktop login (fallback); points to Telegram Desktop folder containing `tdata`.
                    </Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={`${prefix}login_args` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Login Args</Trans>
                  </FormLabel>
                  <FormControl>
                    <ListInput
                      value={field.value || []}
                      onChange={field.onChange}
                      placeholder={t(i18n)`e.g. --namespace my_account`}
                    />
                  </FormControl>
                  <FormDescription className="text-[10px] ml-1">
                    <Trans>
                      Optional extra arguments appended to `tdl login`.
                    </Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>

          <div className="pt-2">
            <FormLabel className="text-xs text-muted-foreground ml-1">
              <Trans>Environment Variables</Trans>
            </FormLabel>
            <EnvVarsFields basePath={pathPrefix || ''} />
          </div>
        </div>

        <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4 shadow-sm">
          <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
            <Terminal className="w-4 h-4 text-blue-500" />
            <h3 className="font-semibold text-sm mr-auto">
              <Trans>Upload Arguments</Trans>
            </h3>
          </div>

          <FormField
            control={control}
            name={`${prefix}args` as any}
            render={({ field }) => (
              <FormItem>
                <FormLabel className="text-xs text-muted-foreground ml-1">
                  <Trans>Command Arguments</Trans>
                </FormLabel>
                <FormControl>
                  <ListInput
                    value={field.value || []}
                    onChange={field.onChange}
                    placeholder={t(i18n)`e.g. up -c @channel -p {input}`}
                  />
                </FormControl>
                <FormDescription className="mt-2 text-sm max-w-full">
                  <div className="p-3 border border-border/40 rounded-lg bg-muted/20">
                    <div className="mb-2 font-semibold text-[10px] uppercase tracking-wide opacity-70">
                      <Trans>Placeholders</Trans>
                    </div>
                    <div className="flex flex-wrap gap-2">
                      {[
                        '{input}',
                        '{streamer}',
                        '{title}',
                        '{platform}',
                        '{streamer_id}',
                        '{session_id}',
                        '{filename}',
                        '{basename}',
                      ].map((p) => (
                        <Badge
                          key={p}
                          variant="outline"
                          className="font-mono text-[10px] bg-background/50 border-border/50"
                        >
                          {p}
                        </Badge>
                      ))}
                    </div>
                  </div>
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
        </div>

        <div className="p-4 rounded-xl bg-muted/10 border border-border/40 space-y-4 shadow-sm">
          <div className="flex items-center gap-2 pb-2 border-b border-border/40 mb-2">
            <Settings className="w-4 h-4 text-orange-500" />
            <h3 className="font-semibold text-sm mr-auto">
              <Trans>Filtering & Behavior</Trans>
            </h3>
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <FormField
              control={control}
              name={`${prefix}upload_all` as any}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 border-border/40 shadow-sm bg-background/50 transition-colors hover:bg-background/80">
                  <div className="space-y-0.5 pr-4">
                    <FormLabel className="text-xs flex items-center gap-1.5">
                      <Upload className="w-3 h-3 text-primary" />
                      <Trans>Upload All Files</Trans>
                    </FormLabel>
                    <FormDescription className="text-[10px]">
                      <Trans>Upload everything regardless of file type</Trans>
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
              name={`${prefix}continue_on_error` as any}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 border-border/40 shadow-sm bg-background/50 transition-colors hover:bg-background/80">
                  <div className="space-y-0.5 pr-4">
                    <FormLabel className="text-xs flex items-center gap-1.5">
                      <AlertTriangle className="w-3 h-3 text-orange-500" />
                      <Trans>Continue on Error</Trans>
                    </FormLabel>
                    <FormDescription className="text-[10px]">
                      <Trans>Keep uploading other files if one fails</Trans>
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

          <div className="grid grid-cols-1 md:grid-cols-2 gap-6 pt-2">
            <FormField
              control={control}
              name={`${prefix}allowed_extensions` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Allowed Extensions</Trans>
                  </FormLabel>
                  <FormControl>
                    <ListInput
                      value={field.value || []}
                      onChange={(v) => field.onChange(v.length ? v : undefined)}
                      placeholder={t(i18n)`e.g. mp4`}
                    />
                  </FormControl>
                  <FormDescription className="text-[10px] ml-1">
                    {uploadAll ? (
                      <Trans>
                        Ignored when "Upload All Files" is enabled.
                      </Trans>
                    ) : (
                      <Trans>
                        List of allowed file extensions (without dot).
                      </Trans>
                    )}
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            <FormField
              control={control}
              name={`${prefix}excluded_extensions` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel className="text-xs text-muted-foreground ml-1">
                    <Trans>Excluded Extensions</Trans>
                  </FormLabel>
                  <FormControl>
                    <ListInput
                      value={field.value || []}
                      onChange={field.onChange}
                      placeholder={t(i18n)`e.g. tmp`}
                    />
                  </FormControl>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-6 pt-2">
            <FormField
              control={control}
              name={`${prefix}max_retries` as any}
              render={({ field }) => (
                <FormItem>
                  <div className="flex items-center gap-2 mb-1">
                    <RefreshCw className="w-3.5 h-3.5 text-blue-500" />
                    <FormLabel className="text-xs text-muted-foreground">
                      <Trans>Max Retries</Trans>
                    </FormLabel>
                  </div>
                  <FormControl>
                    <Input
                      type="number"
                      className="h-11 bg-background/50 border-border/50 focus:bg-background rounded-lg transition-colors"
                      {...field}
                      onChange={(e) => field.onChange(parseInt(e.target.value) || 0)}
                      value={field.value ?? 1}
                    />
                  </FormControl>
                  <FormDescription className="text-[10px] ml-1">
                    <Trans>Number of attempts before failing the upload.</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </div>

          <div className="grid grid-cols-1 md:grid-cols-3 gap-4 pt-2">
            <FormField
              control={control}
              name={`${prefix}include_images` as any}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 border-border/40 shadow-sm bg-background/50 transition-colors hover:bg-background/80">
                  <FormLabel className="text-xs">
                    <Trans>Include Images</Trans>
                  </FormLabel>
                  <FormControl>
                    <Switch
                      checked={field.value}
                      onCheckedChange={field.onChange}
                      disabled={!!uploadAll}
                    />
                  </FormControl>
                </FormItem>
              )}
            />
            <FormField
              control={control}
              name={`${prefix}include_no_extension` as any}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 border-border/40 shadow-sm bg-background/50 transition-colors hover:bg-background/80">
                  <FormLabel className="text-xs text-wrap truncate">
                    <Trans>No Extension</Trans>
                  </FormLabel>
                  <FormControl>
                    <Switch
                      checked={field.value}
                      onCheckedChange={field.onChange}
                      disabled={!!uploadAll}
                    />
                  </FormControl>
                </FormItem>
              )}
            />
            <FormField
              control={control}
              name={`${prefix}dry_run` as any}
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 border-border/40 shadow-sm bg-background/50 transition-colors hover:bg-background/80">
                  <FormLabel className="text-xs">
                    <Trans>Dry Run</Trans>
                  </FormLabel>
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

      <TdlLoginDialog
        open={loginDialogOpen}
        onOpenChange={setLoginDialogOpen}
        tdlPath={tdlPath}
        workingDir={workingDir}
        env={env}
        namespace={namespace}
        storage={storage}
        loginType={loginType}
        allowPassword={false}
        telegramDesktopDir={telegramDesktopDir}
        loginArgs={loginArgs}
      />
    </motion.div >
  );
});
