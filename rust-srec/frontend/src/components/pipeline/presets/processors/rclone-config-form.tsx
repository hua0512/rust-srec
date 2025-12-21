import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
  FormDescription,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { type RcloneConfigSchema } from '../processor-schemas';
import { z } from 'zod';
import { ListInput } from '@/components/ui/list-input';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  Cloud,
  Settings2,
  Terminal,
  ArrowRightLeft,
  Copy,
  Move,
  RefreshCw,
} from 'lucide-react';
import { ProcessorConfigFormProps } from './common-props';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { t } from '@lingui/core/macro';

type RcloneConfig = z.infer<typeof RcloneConfigSchema>;

export function RcloneConfigForm({
  control,
  pathPrefix,
}: ProcessorConfigFormProps<RcloneConfig>) {
  const { i18n } = useLingui();
  const prefix = pathPrefix ? `${pathPrefix}.` : '';

  return (
    <Tabs defaultValue="general" className="w-full">
      <TabsList className="grid w-full grid-cols-2 mb-4 bg-muted/20 p-1">
        <TabsTrigger
          value="general"
          className="data-[state=active]:bg-background data-[state=active]:shadow-sm"
        >
          <Trans>General</Trans>
        </TabsTrigger>
        <TabsTrigger
          value="advanced"
          className="data-[state=active]:bg-background data-[state=active]:shadow-sm"
        >
          <Trans>Advanced</Trans>
        </TabsTrigger>
      </TabsList>

      <TabsContent value="general" className="space-y-4">
        {/* Operation Selection */}
        <Card className="border-border/50 bg-muted/10 shadow-sm">
          <CardHeader className="pb-3 border-b border-border/10 bg-muted/5">
            <div className="flex items-center gap-2">
              <div className="p-1.5 rounded-md bg-background/50 border border-border/20 shadow-sm">
                <ArrowRightLeft className="h-4 w-4 text-primary" />
              </div>
              <CardTitle className="text-sm font-medium">
                <Trans>Operation Mode</Trans>
              </CardTitle>
            </div>
          </CardHeader>
          <CardContent className="grid gap-4 pt-4">
            <FormField
              control={control}
              name={`${prefix}operation` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    <Trans>Operation</Trans>
                  </FormLabel>
                  <Select
                    key={field.value ?? 'initial'}
                    onValueChange={field.onChange}
                    defaultValue={field.value ?? 'copy'}
                  >
                    <FormControl>
                      <SelectTrigger className="h-11 bg-background/50">
                        <div className="flex items-center gap-2">
                          <SelectValue
                            placeholder={t(i18n)`Select operation`}
                          />
                        </div>
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent>
                      <SelectItem value="copy">
                        <div className="flex items-center gap-2">
                          <Copy className="h-4 w-4 text-green-400" />
                          <span>
                            <Trans>Copy</Trans>
                          </span>
                          <span className="ml-2 text-xs text-muted-foreground/50">
                            <Trans>(Preserve source)</Trans>
                          </span>
                        </div>
                      </SelectItem>
                      <SelectItem value="move">
                        <div className="flex items-center gap-2">
                          <Move className="h-4 w-4 text-orange-400" />
                          <span>
                            <Trans>Move</Trans>
                          </span>
                          <span className="ml-2 text-xs text-muted-foreground/50">
                            <Trans>(Delete source)</Trans>
                          </span>
                        </div>
                      </SelectItem>
                      <SelectItem value="sync">
                        <div className="flex items-center gap-2">
                          <RefreshCw className="h-4 w-4 text-blue-400" />
                          <span>
                            <Trans>Sync</Trans>
                          </span>
                          <span className="ml-2 text-xs text-muted-foreground/50">
                            <Trans>(Mirror source)</Trans>
                          </span>
                        </div>
                      </SelectItem>
                    </SelectContent>
                  </Select>
                  <FormDescription>
                    <Trans>
                      Choose how files are transferred to the remote.
                    </Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </CardContent>
        </Card>

        {/* Target Configuration */}
        <Card className="border-border/50 bg-muted/10 shadow-sm">
          <CardHeader className="pb-3 border-b border-border/10 bg-muted/5">
            <div className="flex items-center gap-2">
              <div className="p-1.5 rounded-md bg-background/50 border border-border/20 shadow-sm">
                <Cloud className="h-4 w-4 text-primary" />
              </div>
              <CardTitle className="text-sm font-medium">
                <Trans>Target Configuration</Trans>
              </CardTitle>
            </div>
          </CardHeader>
          <CardContent className="grid gap-4 pt-4">
            <FormField
              control={control}
              name={`${prefix}destination_root` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    <Trans>Destination Root</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      placeholder={t(i18n)`e.g. gdrive:/videos`}
                      {...field}
                      value={field.value ?? ''}
                      className="h-11 bg-background/50 font-mono text-sm"
                    />
                  </FormControl>
                  <FormDescription>
                    <Trans>Base path for remote storage.</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />

            <div className="grid grid-cols-2 gap-4">
              <FormField
                control={control}
                name={`${prefix}config_path` as any}
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>
                      <Trans>Config Path (Optional)</Trans>
                    </FormLabel>
                    <FormControl>
                      <Input
                        placeholder={t(i18n)`/path/to/rclone.conf`}
                        {...field}
                        value={field.value ?? ''}
                        className="bg-background/50"
                      />
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />
              <FormField
                control={control}
                name={`${prefix}rclone_path` as any}
                render={({ field }) => (
                  <FormItem>
                    <FormLabel>
                      <Trans>Rclone Executable</Trans>
                    </FormLabel>
                    <FormControl>
                      <Input
                        {...field}
                        value={field.value ?? ''}
                        className="bg-background/50"
                      />
                    </FormControl>
                    <FormMessage />
                  </FormItem>
                )}
              />
            </div>
          </CardContent>
        </Card>
      </TabsContent>

      <TabsContent value="advanced" className="space-y-4">
        {/* Retry Policy */}
        <Card className="border-border/50 bg-muted/10 shadow-sm">
          <CardHeader className="pb-3 border-b border-border/10 bg-muted/5">
            <div className="flex items-center gap-2">
              <div className="p-1.5 rounded-md bg-background/50 border border-border/20 shadow-sm">
                <Settings2 className="h-4 w-4 text-primary" />
              </div>
              <CardTitle className="text-sm font-medium">
                <Trans>Retry Policy</Trans>
              </CardTitle>
            </div>
          </CardHeader>
          <CardContent className="grid gap-4 pt-4">
            <FormField
              control={control}
              name={`${prefix}max_retries` as any}
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    <Trans>Max Retries</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      type="number"
                      {...field}
                      onChange={(e) => field.onChange(parseInt(e.target.value))}
                      className="bg-background/50"
                    />
                  </FormControl>
                  <FormDescription>
                    <Trans>Number of attempts before failing the upload.</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </CardContent>
        </Card>

        {/* Arguments */}
        <Card className="border-border/50 bg-muted/10 shadow-sm">
          <CardHeader className="pb-3 border-b border-border/10 bg-muted/5">
            <div className="flex items-center gap-2">
              <div className="p-1.5 rounded-md bg-background/50 border border-border/20 shadow-sm">
                <Terminal className="h-4 w-4 text-primary" />
              </div>
              <CardTitle className="text-sm font-medium">
                <Trans>Extra Arguments</Trans>
              </CardTitle>
            </div>
          </CardHeader>
          <CardContent className="pt-4">
            <FormField
              control={control}
              name={`${prefix}args` as any}
              render={({ field }) => (
                <FormItem>
                  <FormControl>
                    <ListInput
                      value={field.value || []}
                      onChange={field.onChange}
                      placeholder={t(i18n)`Add rclone argument`}
                    />
                  </FormControl>
                  <FormDescription>
                    <Trans>Double click to edit items.</Trans>
                  </FormDescription>
                  <FormMessage />
                </FormItem>
              )}
            />
          </CardContent>
        </Card>
      </TabsContent>
    </Tabs>
  );
}
