import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
  FormDescription,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Trans } from '@lingui/react/macro';
import { Switch } from '@/components/ui/switch';
import { Button } from '@/components/ui/button';
import {
  Globe,
  Key,
  User,
  Lock,
  Type,
  Shield,
  Plus,
  Trash2,
  Network,
  List,
} from 'lucide-react';
import { useFormContext, useWatch } from 'react-hook-form';
import { motion, AnimatePresence } from 'motion/react';

export function WebhookForm() {
  const form = useFormContext();
  const authType = useWatch({
    control: form.control,
    name: 'settings.auth.type',
  });

  return (
    <div className="space-y-6">
      {/* General Settings */}
      <Card>
        <CardHeader className="pb-3">
          <div className="flex items-center gap-2">
            <div className="p-2 bg-primary/10 rounded-md">
              <Globe className="h-5 w-5 text-primary" />
            </div>
            <CardTitle className="text-base font-medium">
              <Trans>Endpoint Configuration</Trans>
            </CardTitle>
          </div>
        </CardHeader>
        <CardContent className="grid gap-4">
          <FormField
            control={form.control}
            name="settings.url"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Webhook URL</Trans>
                </FormLabel>
                <FormControl>
                  <div className="relative">
                    <Globe className="absolute left-3 top-2.5 h-4 w-4 text-muted-foreground" />
                    <Input
                      placeholder="https://api.example.com/webhook"
                      {...field}
                      className="pl-9"
                    />
                  </div>
                </FormControl>
                <FormMessage />
              </FormItem>
            )}
          />

          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            <FormField
              control={form.control}
              name="settings.method"
              render={({ field }) => (
                <FormItem>
                  <FormLabel>
                    <Trans>Method</Trans>
                  </FormLabel>
                  <Select
                    onValueChange={field.onChange}
                    defaultValue={field.value}
                  >
                    <FormControl>
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                    </FormControl>
                    <SelectContent>
                      <SelectItem value="POST">POST</SelectItem>
                      <SelectItem value="PUT">PUT</SelectItem>
                    </SelectContent>
                  </Select>
                  <FormMessage />
                </FormItem>
              )}
            />
            <FormField
              control={form.control}
              name="settings.enabled"
              render={({ field }) => (
                <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 shadow-sm h-full">
                  <div className="space-y-0.5">
                    <FormLabel>
                      <Trans>Active</Trans>
                    </FormLabel>
                    <FormDescription>
                      <Trans>Enable or disable this webhook</Trans>
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
        </CardContent>
      </Card>

      {/* Network & Policies */}
      <Card>
        <CardHeader className="pb-3">
          <div className="flex items-center gap-2">
            <div className="p-2 bg-orange-500/10 rounded-md">
              <Network className="h-5 w-5 text-orange-500" />
            </div>
            <CardTitle className="text-base font-medium">
              <Trans>Delivery Policy</Trans>
            </CardTitle>
          </div>
        </CardHeader>
        <CardContent className="grid grid-cols-1 md:grid-cols-2 gap-4">
          <FormField
            control={form.control}
            name="settings.min_priority"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Minimum Priority</Trans>
                </FormLabel>
                <Select
                  onValueChange={field.onChange}
                  defaultValue={field.value}
                >
                  <FormControl>
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                  </FormControl>
                  <SelectContent>
                    <SelectItem value="Low">Low</SelectItem>
                    <SelectItem value="Normal">Normal</SelectItem>
                    <SelectItem value="High">High</SelectItem>
                    <SelectItem value="Critical">Critical</SelectItem>
                  </SelectContent>
                </Select>
                <FormDescription className="text-xs">
                  <Trans>Filter events below this priority</Trans>
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
          <FormField
            control={form.control}
            name="settings.timeout_secs"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Timeout (seconds)</Trans>
                </FormLabel>
                <FormControl>
                  <Input
                    type="number"
                    min={1}
                    {...field}
                    onChange={(e) => field.onChange(e.target.valueAsNumber)}
                  />
                </FormControl>
                <FormDescription className="text-xs">
                  <Trans>Request timeout duration</Trans>
                </FormDescription>
                <FormMessage />
              </FormItem>
            )}
          />
        </CardContent>
      </Card>

      {/* Authentication */}
      <Card>
        <CardHeader className="pb-3">
          <div className="flex items-center gap-2">
            <div className="p-2 bg-blue-500/10 rounded-md">
              <Shield className="h-5 w-5 text-blue-500" />
            </div>
            <CardTitle className="text-base font-medium">
              <Trans>Authentication</Trans>
            </CardTitle>
          </div>
        </CardHeader>
        <CardContent className="space-y-4">
          <FormField
            control={form.control}
            name="settings.auth.type"
            render={({ field }) => (
              <FormItem>
                <FormLabel>
                  <Trans>Auth Type</Trans>
                </FormLabel>
                <Select
                  onValueChange={(value) => {
                    const form_ctx = form as any;
                    if (value === 'None') {
                      form_ctx.setValue('settings.auth', { type: 'None' });
                    } else if (value === 'Bearer') {
                      form_ctx.setValue('settings.auth', {
                        type: 'Bearer',
                        token: '',
                      });
                    } else if (value === 'Basic') {
                      form_ctx.setValue('settings.auth', {
                        type: 'Basic',
                        username: '',
                        password: '',
                      });
                    } else if (value === 'Header') {
                      form_ctx.setValue('settings.auth', {
                        type: 'Header',
                        name: '',
                        value: '',
                      });
                    }
                  }}
                  defaultValue={field.value}
                >
                  <FormControl>
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                  </FormControl>
                  <SelectContent>
                    <SelectItem value="None">
                      <Trans>None</Trans>
                    </SelectItem>
                    <SelectItem value="Bearer">
                      <Trans>Bearer Token</Trans>
                    </SelectItem>
                    <SelectItem value="Basic">
                      <Trans>Basic Auth</Trans>
                    </SelectItem>
                    <SelectItem value="Header">
                      <Trans>Custom Header</Trans>
                    </SelectItem>
                  </SelectContent>
                </Select>
                <FormMessage />
              </FormItem>
            )}
          />

          <AnimatePresence mode="wait">
            {authType === 'Bearer' && (
              <motion.div
                key="bearer"
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: 'auto' }}
                exit={{ opacity: 0, height: 0 }}
                className="overflow-hidden"
              >
                <FormField
                  control={form.control}
                  name="settings.auth.token"
                  render={({ field }) => (
                    <FormItem>
                      <FormLabel>
                        <Trans>Token</Trans>
                      </FormLabel>
                      <FormControl>
                        <div className="relative">
                          <Key className="absolute left-3 top-2.5 h-4 w-4 text-muted-foreground" />
                          <Input
                            type="password"
                            placeholder="ey..."
                            {...field}
                            className="pl-9"
                          />
                        </div>
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />
              </motion.div>
            )}

            {authType === 'Basic' && (
              <motion.div
                key="basic"
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: 'auto' }}
                exit={{ opacity: 0, height: 0 }}
                className="overflow-hidden"
              >
                <div className="grid grid-cols-2 gap-4">
                  <FormField
                    control={form.control}
                    name="settings.auth.username"
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel>
                          <Trans>Username</Trans>
                        </FormLabel>
                        <FormControl>
                          <div className="relative">
                            <User className="absolute left-3 top-2.5 h-4 w-4 text-muted-foreground" />
                            <Input {...field} className="pl-9" />
                          </div>
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                  <FormField
                    control={form.control}
                    name="settings.auth.password"
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel>
                          <Trans>Password</Trans>
                        </FormLabel>
                        <FormControl>
                          <div className="relative">
                            <Lock className="absolute left-3 top-2.5 h-4 w-4 text-muted-foreground" />
                            <Input
                              type="password"
                              {...field}
                              className="pl-9"
                            />
                          </div>
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                </div>
              </motion.div>
            )}

            {authType === 'Header' && (
              <motion.div
                key="header"
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: 'auto' }}
                exit={{ opacity: 0, height: 0 }}
                className="overflow-hidden"
              >
                <div className="grid grid-cols-2 gap-4">
                  <FormField
                    control={form.control}
                    name="settings.auth.name"
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel>
                          <Trans>Header Key</Trans>
                        </FormLabel>
                        <FormControl>
                          <div className="relative">
                            <Type className="absolute left-3 top-2.5 h-4 w-4 text-muted-foreground" />
                            <Input
                              placeholder="X-Auth-Key"
                              {...field}
                              className="pl-9"
                            />
                          </div>
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                  <FormField
                    control={form.control}
                    name="settings.auth.value"
                    render={({ field }) => (
                      <FormItem>
                        <FormLabel>
                          <Trans>Header Value</Trans>
                        </FormLabel>
                        <FormControl>
                          <div className="relative">
                            <Shield className="absolute left-3 top-2.5 h-4 w-4 text-muted-foreground" />
                            <Input
                              type="password"
                              placeholder="secret"
                              {...field}
                              className="pl-9"
                            />
                          </div>
                        </FormControl>
                        <FormMessage />
                      </FormItem>
                    )}
                  />
                </div>
              </motion.div>
            )}
          </AnimatePresence>
        </CardContent>
      </Card>

      {/* Headers List */}
      <Card>
        <CardHeader className="pb-3 flex flex-row items-center justify-between">
          <div className="flex items-center gap-2">
            <div className="p-2 bg-purple-500/10 rounded-md">
              <List className="h-5 w-5 text-purple-500" />
            </div>
            <CardTitle className="text-base font-medium">
              <Trans>Custom Headers</Trans>
            </CardTitle>
          </div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="hover:bg-primary/10 hover:text-primary"
            onClick={() => {
              const currentHeaders = form.getValues('settings.headers') || [];
              form.setValue('settings.headers', [...currentHeaders, ['', '']]);
            }}
          >
            <Plus className="h-4 w-4 mr-1" />
            <Trans>Add</Trans>
          </Button>
        </CardHeader>
        <CardContent>
          <div className="space-y-3">
            {(
              useWatch({ control: form.control, name: 'settings.headers' }) ||
              []
            ).length === 0 && (
              <div className="text-center py-6 text-muted-foreground text-sm border-2 border-dashed rounded-lg">
                <Trans>No custom headers configured</Trans>
              </div>
            )}
            {(
              useWatch({ control: form.control, name: 'settings.headers' }) ||
              []
            ).map((_: any, index: number) => (
              <div key={index} className="flex gap-2 items-start group">
                <FormField
                  control={form.control}
                  name={`settings.headers.${index}.0`}
                  render={({ field }) => (
                    <FormItem className="flex-1">
                      <FormControl>
                        <Input {...field} placeholder="Key" />
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />
                <FormField
                  control={form.control}
                  name={`settings.headers.${index}.1`}
                  render={({ field }) => (
                    <FormItem className="flex-1">
                      <FormControl>
                        <Input {...field} placeholder="Value" />
                      </FormControl>
                      <FormMessage />
                    </FormItem>
                  )}
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="text-muted-foreground hover:text-destructive transition-colors"
                  onClick={() => {
                    const currentHeaders = form.getValues('settings.headers');
                    form.setValue(
                      'settings.headers',
                      currentHeaders.filter((_: any, i: number) => i !== index),
                    );
                  }}
                >
                  <Trash2 className="h-4 w-4" />
                </Button>
              </div>
            ))}
          </div>
        </CardContent>
      </Card>
    </div>
  );
}
