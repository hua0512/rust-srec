import { UseFormReturn } from 'react-hook-form';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../../ui/tabs';
import { Card, CardContent } from '../../ui/card';
import {
  FormControl,
  FormDescription,
  FormItem,
  FormLabel,
} from '../../ui/form';
import { Input } from '../../ui/input';
import { Switch } from '../../ui/switch';
import { Trans } from '@lingui/react/macro';
import { useEffect, useState } from 'react';
import {
  StreamSelectionInput,
  StreamSelectionConfig,
} from './stream-selection-input';
import { RetryPolicyForm } from './retry-policy-form';
import { DanmuConfigForm } from './danmu-config-form';
import {
  Filter,
  FolderOutput,
  Network,
  MessageSquare,
  Cookie,
  Shield,
  Webhook,
  Workflow,
} from 'lucide-react';
import { PipelineEditor } from '../../pipeline/editor/pipeline-editor';

interface ProxyConfig {
  enabled?: boolean;
  url?: string;
  username?: string;
  password?: string;
  use_system_proxy?: boolean;
}

interface EventHooks {
  on_online?: string;
  on_offline?: string;
  on_download_start?: string;
  on_download_complete?: string;
  on_download_error?: string;
  on_pipeline_complete?: string;
}

interface StreamerSpecificConfig {
  output_folder?: string;
  output_filename_template?: string;
  max_part_size_bytes?: number;
  max_download_duration_secs?: number;
  cookies?: string;
  output_file_format?: string;
  min_segment_size_bytes?: number;
  download_engine?: string;
  record_danmu?: boolean;
  stream_selection?: StreamSelectionConfig;
  proxy_config?: ProxyConfig;
  event_hooks?: EventHooks;
  pipeline?: any[];
}

interface StreamerConfigurationProps {
  form: UseFormReturn<any>;
}

export function StreamerConfiguration({ form }: StreamerConfigurationProps) {
  const specificConfigJson = form.watch('streamer_specific_config');
  const [specificConfig, setSpecificConfig] = useState<StreamerSpecificConfig>(
    {},
  );

  useEffect(() => {
    if (specificConfigJson) {
      try {
        const parsed = JSON.parse(specificConfigJson);
        setSpecificConfig(parsed);
      } catch (e) {
        // ignore invalid json
      }
    } else {
      setSpecificConfig({});
    }
  }, []);

  const updateSpecificConfig = (newConfig: StreamerSpecificConfig) => {
    setSpecificConfig(newConfig);
    const cleanConfig = JSON.parse(JSON.stringify(newConfig));
    if (Object.keys(cleanConfig).length === 0) {
      form.setValue('streamer_specific_config', undefined, {
        shouldDirty: true,
        shouldValidate: true,
      });
    } else {
      form.setValue('streamer_specific_config', JSON.stringify(cleanConfig), {
        shouldDirty: true,
        shouldValidate: true,
      });
    }
  };

  const updateSpecificField = (
    key: keyof StreamerSpecificConfig,
    value: any,
  ) => {
    const newConfig = { ...specificConfig };
    if (
      value === '' ||
      value === undefined ||
      (typeof value === 'object' && Object.keys(value).length === 0)
    ) {
      delete newConfig[key];
    } else {
      // @ts-ignore
      newConfig[key] = value;
    }
    updateSpecificConfig(newConfig);
  };

  const updateProxyField = (key: keyof ProxyConfig, value: any) => {
    const newProxy = { ...(specificConfig.proxy_config || {}) };
    if (value === '' || value === undefined || value === false) {
      delete newProxy[key];
    } else {
      newProxy[key] = value;
    }
    updateSpecificField(
      'proxy_config',
      Object.keys(newProxy).length > 0 ? newProxy : undefined,
    );
  };

  const updateEventHook = (key: keyof EventHooks, value: string) => {
    const newHooks = { ...(specificConfig.event_hooks || {}) };
    if (value === '') {
      delete newHooks[key];
    } else {
      newHooks[key] = value;
    }
    updateSpecificField(
      'event_hooks',
      Object.keys(newHooks).length > 0 ? newHooks : undefined,
    );
  };

  return (
    <Tabs defaultValue="filters" className="w-full">
      <TabsList className="grid w-full grid-cols-3 sm:grid-cols-7 h-auto">
        <TabsTrigger value="filters" className="flex items-center gap-2">
          <Filter className="w-4 h-4" />{' '}
          <span className="hidden sm:inline">
            <Trans>Filters</Trans>
          </span>
        </TabsTrigger>
        <TabsTrigger value="output" className="flex items-center gap-2">
          <FolderOutput className="w-4 h-4" />{' '}
          <span className="hidden sm:inline">
            <Trans>Output</Trans>
          </span>
        </TabsTrigger>
        <TabsTrigger value="network" className="flex items-center gap-2">
          <Network className="w-4 h-4" />{' '}
          <span className="hidden sm:inline">
            <Trans>Network</Trans>
          </span>
        </TabsTrigger>
        <TabsTrigger value="proxy" className="flex items-center gap-2">
          <Shield className="w-4 h-4" />{' '}
          <span className="hidden sm:inline">
            <Trans>Proxy</Trans>
          </span>
        </TabsTrigger>
        <TabsTrigger value="hooks" className="flex items-center gap-2">
          <Webhook className="w-4 h-4" />{' '}
          <span className="hidden sm:inline">
            <Trans>Hooks</Trans>
          </span>
        </TabsTrigger>
        <TabsTrigger value="danmu" className="flex items-center gap-2">
          <MessageSquare className="w-4 h-4" />{' '}
          <span className="hidden sm:inline">
            <Trans>Danmu</Trans>
          </span>
        </TabsTrigger>
        <TabsTrigger value="pipeline" className="flex items-center gap-2">
          <Workflow className="w-4 h-4" />{' '}
          <span className="hidden sm:inline">
            <Trans>Pipeline</Trans>
          </span>
        </TabsTrigger>
      </TabsList>

      <div className="mt-4">
        <TabsContent value="filters">
          <Card className="border-dashed shadow-none">
            <CardContent className="pt-6">
              <StreamSelectionInput
                value={specificConfig.stream_selection}
                onChange={(val) => updateSpecificField('stream_selection', val)}
              />
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="output">
          <Card className="border-dashed shadow-none">
            <CardContent className="pt-6 space-y-4">
              <FormItem>
                <FormLabel>
                  <Trans>Output Folder</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    placeholder="/path/to/downloads"
                    value={specificConfig.output_folder || ''}
                    onChange={(e) =>
                      updateSpecificField('output_folder', e.target.value)
                    }
                    className="font-mono text-sm"
                  />
                </FormControl>
                <FormDescription>
                  <Trans>Override the default download directory.</Trans>
                </FormDescription>
              </FormItem>

              <FormItem>
                <FormLabel>
                  <Trans>Filename Template</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    placeholder="{streamer} - {title} - {time}"
                    value={specificConfig.output_filename_template || ''}
                    onChange={(e) =>
                      updateSpecificField(
                        'output_filename_template',
                        e.target.value,
                      )
                    }
                    className="font-mono text-sm"
                  />
                </FormControl>
                <FormDescription>
                  <Trans>Custom filename pattern.</Trans>
                </FormDescription>
              </FormItem>

              <FormItem>
                <FormLabel>
                  <Trans>Output Format</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    placeholder="flv, mp4, ts, mkv"
                    value={specificConfig.output_file_format || ''}
                    onChange={(e) =>
                      updateSpecificField('output_file_format', e.target.value)
                    }
                    className="font-mono text-sm"
                  />
                </FormControl>
                <FormDescription>
                  <Trans>File format extension.</Trans>
                </FormDescription>
              </FormItem>

              <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                <FormItem>
                  <FormLabel>
                    <Trans>Download Engine</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      placeholder="mesio, ffmpeg, streamlink"
                      value={specificConfig.download_engine || ''}
                      onChange={(e) =>
                        updateSpecificField('download_engine', e.target.value)
                      }
                      className="font-mono text-sm"
                    />
                  </FormControl>
                  <FormDescription>
                    <Trans>Engine backend.</Trans>
                  </FormDescription>
                </FormItem>
                <FormItem>
                  <FormLabel>
                    <Trans>Min Segment Size (Bytes)</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      type="number"
                      min={0}
                      placeholder="e.g. 1048576"
                      value={specificConfig.min_segment_size_bytes ?? ''}
                      onChange={(e) =>
                        updateSpecificField(
                          'min_segment_size_bytes',
                          e.target.value ? parseInt(e.target.value) : undefined,
                        )
                      }
                      className="font-mono text-sm"
                    />
                  </FormControl>
                  <FormDescription>
                    <Trans>Discard small segments.</Trans>
                  </FormDescription>
                </FormItem>
              </div>

              <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                <FormItem>
                  <FormLabel>
                    <Trans>Max Part Size (Bytes)</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      type="number"
                      min={0}
                      placeholder="e.g. 1073741824"
                      value={specificConfig.max_part_size_bytes ?? ''}
                      onChange={(e) =>
                        updateSpecificField(
                          'max_part_size_bytes',
                          e.target.value ? parseInt(e.target.value) : undefined,
                        )
                      }
                      className="font-mono text-sm"
                    />
                  </FormControl>
                  <FormDescription>
                    <Trans>Split file when size exceeded.</Trans>
                  </FormDescription>
                </FormItem>

                <FormItem>
                  <FormLabel>
                    <Trans>Max Duration (s)</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      type="number"
                      min={0}
                      placeholder="e.g. 3600"
                      value={specificConfig.max_download_duration_secs ?? ''}
                      onChange={(e) =>
                        updateSpecificField(
                          'max_download_duration_secs',
                          e.target.value ? parseInt(e.target.value) : undefined,
                        )
                      }
                      className="font-mono text-sm"
                    />
                  </FormControl>
                  <FormDescription>
                    <Trans>Split file when duration exceeded.</Trans>
                  </FormDescription>
                </FormItem>
              </div>
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="network">
          <Card className="border-dashed shadow-none">
            <CardContent className="pt-6 space-y-6">
              <FormItem>
                <FormLabel className="flex items-center gap-2">
                  <Cookie className="w-4 h-4" /> <Trans>Cookies</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    placeholder="key=value; key2=value2"
                    value={specificConfig.cookies || ''}
                    onChange={(e) =>
                      updateSpecificField('cookies', e.target.value)
                    }
                    className="font-mono text-xs"
                  />
                </FormControl>
                <FormDescription>
                  <Trans>HTTP cookies for authentication (if required).</Trans>
                </FormDescription>
              </FormItem>

              <div className="space-y-4">
                <h4 className="text-sm font-medium">
                  <Trans>Download Retry Policy</Trans>
                </h4>
                <RetryPolicyForm form={form} name="download_retry_policy" />
              </div>
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="proxy">
          <Card className="border-dashed shadow-none">
            <CardContent className="pt-6 space-y-4">
              <FormItem className="flex flex-row items-center justify-between rounded-lg border p-4">
                <div className="space-y-0.5">
                  <FormLabel>
                    <Trans>Enable Proxy</Trans>
                  </FormLabel>
                  <FormDescription>
                    <Trans>Use a proxy server for this streamer.</Trans>
                  </FormDescription>
                </div>
                <FormControl>
                  <Switch
                    checked={specificConfig.proxy_config?.enabled || false}
                    onCheckedChange={(checked) =>
                      updateProxyField('enabled', checked)
                    }
                  />
                </FormControl>
              </FormItem>

              <FormItem className="flex flex-row items-center justify-between rounded-lg border p-4">
                <div className="space-y-0.5">
                  <FormLabel>
                    <Trans>Use System Proxy</Trans>
                  </FormLabel>
                  <FormDescription>
                    <Trans>Use system-configured proxy settings.</Trans>
                  </FormDescription>
                </div>
                <FormControl>
                  <Switch
                    checked={
                      specificConfig.proxy_config?.use_system_proxy || false
                    }
                    onCheckedChange={(checked) =>
                      updateProxyField('use_system_proxy', checked)
                    }
                  />
                </FormControl>
              </FormItem>

              <FormItem>
                <FormLabel>
                  <Trans>Proxy URL</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    placeholder="http://proxy.example.com:8080"
                    value={specificConfig.proxy_config?.url || ''}
                    onChange={(e) => updateProxyField('url', e.target.value)}
                    className="font-mono text-sm"
                  />
                </FormControl>
                <FormDescription>
                  <Trans>HTTP/HTTPS/SOCKS5 proxy URL.</Trans>
                </FormDescription>
              </FormItem>

              <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                <FormItem>
                  <FormLabel>
                    <Trans>Username</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      placeholder="proxy_user"
                      value={specificConfig.proxy_config?.username || ''}
                      onChange={(e) =>
                        updateProxyField('username', e.target.value)
                      }
                    />
                  </FormControl>
                </FormItem>
                <FormItem>
                  <FormLabel>
                    <Trans>Password</Trans>
                  </FormLabel>
                  <FormControl>
                    <Input
                      type="password"
                      placeholder="********"
                      value={specificConfig.proxy_config?.password || ''}
                      onChange={(e) =>
                        updateProxyField('password', e.target.value)
                      }
                    />
                  </FormControl>
                </FormItem>
              </div>
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="hooks">
          <Card className="border-dashed shadow-none">
            <CardContent className="pt-6 space-y-4">
              <p className="text-sm text-muted-foreground mb-4">
                <Trans>
                  Execute shell commands on lifecycle events. Commands run in
                  the system shell.
                </Trans>
              </p>

              <FormItem>
                <FormLabel>
                  <Trans>On Online</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    placeholder="notify-send 'Streamer is online!'"
                    value={specificConfig.event_hooks?.on_online || ''}
                    onChange={(e) =>
                      updateEventHook('on_online', e.target.value)
                    }
                    className="font-mono text-sm"
                  />
                </FormControl>
                <FormDescription>
                  <Trans>Runs when the streamer goes online.</Trans>
                </FormDescription>
              </FormItem>

              <FormItem>
                <FormLabel>
                  <Trans>On Offline</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    placeholder="echo 'Streamer went offline'"
                    value={specificConfig.event_hooks?.on_offline || ''}
                    onChange={(e) =>
                      updateEventHook('on_offline', e.target.value)
                    }
                    className="font-mono text-sm"
                  />
                </FormControl>
                <FormDescription>
                  <Trans>Runs when the streamer goes offline.</Trans>
                </FormDescription>
              </FormItem>

              <FormItem>
                <FormLabel>
                  <Trans>On Download Start</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    placeholder="echo 'Download started'"
                    value={specificConfig.event_hooks?.on_download_start || ''}
                    onChange={(e) =>
                      updateEventHook('on_download_start', e.target.value)
                    }
                    className="font-mono text-sm"
                  />
                </FormControl>
                <FormDescription>
                  <Trans>Runs when a download begins.</Trans>
                </FormDescription>
              </FormItem>

              <FormItem>
                <FormLabel>
                  <Trans>On Download Complete</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    placeholder="echo 'Download finished: $FILE_PATH'"
                    value={
                      specificConfig.event_hooks?.on_download_complete || ''
                    }
                    onChange={(e) =>
                      updateEventHook('on_download_complete', e.target.value)
                    }
                    className="font-mono text-sm"
                  />
                </FormControl>
                <FormDescription>
                  <Trans>Runs when a download completes successfully.</Trans>
                </FormDescription>
              </FormItem>

              <FormItem>
                <FormLabel>
                  <Trans>On Download Error</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    placeholder="echo 'Download failed: $ERROR'"
                    value={specificConfig.event_hooks?.on_download_error || ''}
                    onChange={(e) =>
                      updateEventHook('on_download_error', e.target.value)
                    }
                    className="font-mono text-sm"
                  />
                </FormControl>
                <FormDescription>
                  <Trans>Runs when a download encounters an error.</Trans>
                </FormDescription>
              </FormItem>

              <FormItem>
                <FormLabel>
                  <Trans>On Pipeline Complete</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    placeholder="echo 'Pipeline finished'"
                    value={
                      specificConfig.event_hooks?.on_pipeline_complete || ''
                    }
                    onChange={(e) =>
                      updateEventHook('on_pipeline_complete', e.target.value)
                    }
                    className="font-mono text-sm"
                  />
                </FormControl>
                <FormDescription>
                  <Trans>
                    Runs when the post-processing pipeline completes.
                  </Trans>
                </FormDescription>
              </FormItem>
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="danmu">
          <Card className="border-dashed shadow-none">
            <CardContent className="pt-6">
              <div className="border rounded-md p-4">
                <h4 className="mb-4 text-sm font-medium">
                  <Trans>Danmu Configuration Override</Trans>
                </h4>
                <div className="space-y-4">
                  <FormItem>
                    <FormLabel>
                      <Trans>Recording Status</Trans>
                    </FormLabel>
                    <FormControl>
                      <div className="relative">
                        <select
                          className="flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background file:border-0 file:bg-transparent file:text-sm file:font-medium placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50"
                          value={
                            specificConfig.record_danmu === undefined
                              ? 'inherit'
                              : specificConfig.record_danmu.toString()
                          }
                          onChange={(e) => {
                            const val = e.target.value;
                            if (val === 'inherit') {
                              updateSpecificField('record_danmu', undefined);
                            } else {
                              updateSpecificField(
                                'record_danmu',
                                val === 'true',
                              );
                            }
                          }}
                        >
                          <option value="inherit">Inherit</option>
                          <option value="true">Enabled</option>
                          <option value="false">Disabled</option>
                        </select>
                      </div>
                    </FormControl>
                    <FormDescription>
                      <Trans>
                        Enable or disable danmu recording explicitly.
                      </Trans>
                    </FormDescription>
                  </FormItem>
                </div>
              </div>

              <DanmuConfigForm form={form} name="danmu_sampling_config" />
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="pipeline">
          <Card className="border-dashed shadow-none">
            <CardContent className="pt-6">
              <PipelineEditor
                steps={specificConfig.pipeline || []}
                onChange={(steps) => updateSpecificField('pipeline', steps)}
              />
            </CardContent>
          </Card>
        </TabsContent>
      </div>
    </Tabs>
  );
}
