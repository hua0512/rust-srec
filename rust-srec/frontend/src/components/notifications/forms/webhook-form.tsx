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
import { Card, CardContent } from '@/components/ui/card';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
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
import { memo } from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import { motion, AnimatePresence } from 'motion/react';
import { IconInput } from '@/components/ui/icon-input';
import { SwitchCard } from '@/components/ui/switch-card';
import { CardHeaderWithIcon } from '@/components/ui/card-header-with-icon';

export const WebhookForm = memo(function WebhookForm() {
  const { i18n } = useLingui();
  const form = useFormContext();
  const authType = useWatch({
    control: form.control,
    name: 'settings.auth.type',
  });
  // Single useWatch for headers to avoid double subscription
  const headers =
    useWatch({
      control: form.control,
      name: 'settings.headers',
    }) || [];

  return (
    <div className="space-y-6">
      {/* General Settings */}
      <Card>
        <CardHeaderWithIcon
          icon={Globe}
          title={<Trans>Endpoint Configuration</Trans>}
        />
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
                  <IconInput
                    icon={Globe}
                    placeholder={i18n._(msg`https://api.example.com/webhook`)}
                    {...field}
                  />
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
                <SwitchCard
                  label={<Trans>Active</Trans>}
                  description={<Trans>Enable or disable this webhook</Trans>}
                  checked={field.value}
                  onCheckedChange={field.onChange}
                  className="h-full"
                />
              )}
            />
          </div>
        </CardContent>
      </Card>

      {/* Network & Policies */}
      <Card>
        <CardHeaderWithIcon
          icon={Network}
          title={<Trans>Delivery Policy</Trans>}
          iconClassName="text-orange-500"
          iconBgClassName="bg-orange-500/10"
        />
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
                    <SelectItem value="Low">
                      <Trans>Low</Trans>
                    </SelectItem>
                    <SelectItem value="Normal">
                      <Trans>Normal</Trans>
                    </SelectItem>
                    <SelectItem value="High">
                      <Trans>High</Trans>
                    </SelectItem>
                    <SelectItem value="Critical">
                      <Trans>Critical</Trans>
                    </SelectItem>
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
        <CardHeaderWithIcon
          icon={Shield}
          title={<Trans>Authentication</Trans>}
          iconClassName="text-blue-500"
          iconBgClassName="bg-blue-500/10"
        />
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
                        <IconInput
                          icon={Key}
                          type="password"
                          placeholder={i18n._(msg`ey...`)}
                          {...field}
                        />
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
                          <IconInput
                            icon={User}
                            placeholder={i18n._(msg`Username`)}
                            {...field}
                          />
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
                          <IconInput
                            icon={Lock}
                            type="password"
                            placeholder={i18n._(msg`Password`)}
                            {...field}
                          />
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
                          <IconInput
                            icon={Type}
                            placeholder={i18n._(msg`X-Auth-Key`)}
                            {...field}
                          />
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
                          <IconInput
                            icon={Shield}
                            type="password"
                            placeholder={i18n._(msg`secret`)}
                            {...field}
                          />
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
        <CardHeaderWithIcon
          icon={List}
          title={<Trans>Custom Headers</Trans>}
          iconClassName="text-purple-500"
          iconBgClassName="bg-purple-500/10"
          className="flex flex-row items-center justify-between"
        >
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
        </CardHeaderWithIcon>
        <CardContent>
          <div className="space-y-3">
            {headers.length === 0 && (
              <div className="text-center py-6 text-muted-foreground text-sm border-2 border-dashed rounded-lg">
                <Trans>No custom headers configured</Trans>
              </div>
            )}
            {headers.map((_: any, index: number) => (
              <div key={index} className="flex gap-2 items-start group">
                <FormField
                  control={form.control}
                  name={`settings.headers.${index}.0`}
                  render={({ field }) => (
                    <FormItem className="flex-1">
                      <FormControl>
                        <Input {...field} placeholder={i18n._(msg`Key`)} />
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
                        <Input {...field} placeholder={i18n._(msg`Value`)} />
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
});
