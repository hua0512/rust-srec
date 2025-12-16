import {
    FormControl,
    FormField,
    FormItem,
    FormLabel,
    FormMessage,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from '@/components/ui/select';
import { Trans } from '@lingui/react/macro';
import { Globe, Key, User, Lock, Type, Shield } from 'lucide-react';
import { useFormContext, useWatch } from 'react-hook-form';
import { motion, AnimatePresence } from 'motion/react';

export function WebhookForm() {
    const form = useFormContext();
    const authType = useWatch({ control: form.control, name: 'settings.auth.type' });

    return (
        <div className="space-y-4 rounded-xl border border-primary/10 bg-primary/5 p-4">
            <FormField
                control={form.control}
                name="settings.url"
                render={({ field }) => (
                    <FormItem>
                        <FormLabel><Trans>URL</Trans></FormLabel>
                        <FormControl>
                            <div className="relative">
                                <Globe className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                                <Input placeholder="https://..." {...field} className="pl-9 bg-background/50" />
                            </div>
                        </FormControl>
                        <FormMessage />
                    </FormItem>
                )}
            />
            <FormField
                control={form.control}
                name="settings.method"
                render={({ field }) => (
                    <FormItem>
                        <FormLabel><Trans>Method</Trans></FormLabel>
                        <Select
                            onValueChange={field.onChange}
                            defaultValue={field.value}
                        >
                            <FormControl>
                                <SelectTrigger className="bg-background/50">
                                    <SelectValue />
                                </SelectTrigger>
                            </FormControl>
                            <SelectContent>
                                <SelectItem value="POST">POST</SelectItem>
                                <SelectItem value="PUT">PUT</SelectItem>
                                <SelectItem value="GET">GET</SelectItem>
                            </SelectContent>
                        </Select>
                        <FormMessage />
                    </FormItem>
                )}
            />

            <div className="pt-2">
                <FormField
                    control={form.control}
                    name="settings.auth.type"
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel><Trans>Authentication</Trans></FormLabel>
                            <Select
                                onValueChange={(value) => {
                                    // When changing auth type, reset the entire auth object
                                    const form_ctx = form as any;
                                    if (value === 'None') {
                                        form_ctx.setValue('settings.auth', { type: 'None' });
                                    } else if (value === 'Bearer') {
                                        form_ctx.setValue('settings.auth', { type: 'Bearer', token: '' });
                                    } else if (value === 'Basic') {
                                        form_ctx.setValue('settings.auth', { type: 'Basic', username: '', password: '' });
                                    } else if (value === 'Header') {
                                        form_ctx.setValue('settings.auth', { type: 'Header', name: '', value: '' });
                                    }
                                }}
                                defaultValue={field.value}
                            >
                                <FormControl>
                                    <SelectTrigger className="bg-background/50">
                                        <SelectValue />
                                    </SelectTrigger>
                                </FormControl>
                                <SelectContent>
                                    <SelectItem value="None"><Trans>None</Trans></SelectItem>
                                    <SelectItem value="Bearer"><Trans>Bearer Token</Trans></SelectItem>
                                    <SelectItem value="Basic"><Trans>Basic Auth</Trans></SelectItem>
                                    <SelectItem value="Header"><Trans>Custom Header</Trans></SelectItem>
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
                            transition={{ duration: 0.2 }}
                            className="overflow-hidden"
                        >
                            <div className="pt-4">
                                <FormField
                                    control={form.control}
                                    name="settings.auth.token"
                                    render={({ field }) => (
                                        <FormItem>
                                            <FormLabel><Trans>Token</Trans></FormLabel>
                                            <FormControl>
                                                <div className="relative">
                                                    <Key className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                                                    <Input type="password" placeholder="ey..." {...field} className="pl-9 bg-background/50" />
                                                </div>
                                            </FormControl>
                                            <FormMessage />
                                        </FormItem>
                                    )}
                                />
                            </div>
                        </motion.div>
                    )}

                    {authType === 'Basic' && (
                        <motion.div
                            key="basic"
                            initial={{ opacity: 0, height: 0 }}
                            animate={{ opacity: 1, height: 'auto' }}
                            exit={{ opacity: 0, height: 0 }}
                            transition={{ duration: 0.2 }}
                            className="overflow-hidden"
                        >
                            <div className="pt-4 grid grid-cols-2 gap-4">
                                <FormField
                                    control={form.control}
                                    name="settings.auth.username"
                                    render={({ field }) => (
                                        <FormItem>
                                            <FormLabel><Trans>Username</Trans></FormLabel>
                                            <FormControl>
                                                <div className="relative">
                                                    <User className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                                                    <Input placeholder="username" {...field} className="pl-9 bg-background/50" />
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
                                            <FormLabel><Trans>Password</Trans></FormLabel>
                                            <FormControl>
                                                <div className="relative">
                                                    <Lock className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                                                    <Input type="password" placeholder="password" {...field} className="pl-9 bg-background/50" />
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
                            transition={{ duration: 0.2 }}
                            className="overflow-hidden"
                        >
                            <div className="pt-4 grid grid-cols-2 gap-4">
                                <FormField
                                    control={form.control}
                                    name="settings.auth.name"
                                    render={({ field }) => (
                                        <FormItem>
                                            <FormLabel><Trans>Header Name</Trans></FormLabel>
                                            <FormControl>
                                                <div className="relative">
                                                    <Type className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                                                    <Input placeholder="X-Custom-Auth" {...field} className="pl-9 bg-background/50" />
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
                                            <FormLabel><Trans>Header Value</Trans></FormLabel>
                                            <FormControl>
                                                <div className="relative">
                                                    <Shield className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                                                    <Input type="password" placeholder="secret-value" {...field} className="pl-9 bg-background/50" />
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
            </div>
        </div>
    );
}
