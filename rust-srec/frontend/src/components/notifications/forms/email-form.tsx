import {
    FormControl,
    FormField,
    FormItem,
    FormLabel,
    FormMessage,
    FormDescription,
} from '@/components/ui/form';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { TagInput } from '@/components/ui/tag-input';
import { Trans } from '@lingui/react/macro';
import { Globe, Hash, User, Shield, Mail } from 'lucide-react';
import { useFormContext } from 'react-hook-form';

export function EmailForm() {
    const form = useFormContext();

    return (
        <div className="space-y-4 rounded-xl border border-primary/10 bg-primary/5 p-4">
            <div className="grid grid-cols-3 gap-4">
                <div className="col-span-2">
                    <FormField
                        control={form.control}
                        name="smtp_host"
                        render={({ field }) => (
                            <FormItem>
                                <FormLabel><Trans>SMTP Host</Trans></FormLabel>
                                <FormControl>
                                    <div className="relative">
                                        <Globe className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                                        <Input placeholder="smtp.gmail.com" {...field} className="pl-9 bg-background/50" />
                                    </div>
                                </FormControl>
                                <FormMessage />
                            </FormItem>
                        )}
                    />
                </div>
                <FormField
                    control={form.control}
                    name="smtp_port"
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel><Trans>Port</Trans></FormLabel>
                            <FormControl>
                                <div className="relative">
                                    <Hash className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                                    <Input
                                        type="number"
                                        placeholder="587"
                                        {...field}
                                        onChange={e => field.onChange(e.target.valueAsNumber)}
                                        className="pl-9 bg-background/50"
                                    />
                                </div>
                            </FormControl>
                            <FormMessage />
                        </FormItem>
                    )}
                />
            </div>

            <div className="grid grid-cols-2 gap-4">
                <FormField
                    control={form.control}
                    name="email_username"
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel><Trans>Username</Trans></FormLabel>
                            <FormControl>
                                <div className="relative">
                                    <User className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                                    <Input {...field} className="pl-9 bg-background/50" />
                                </div>
                            </FormControl>
                            <FormMessage />
                        </FormItem>
                    )}
                />
                <FormField
                    control={form.control}
                    name="email_password"
                    render={({ field }) => (
                        <FormItem>
                            <FormLabel><Trans>Password</Trans></FormLabel>
                            <FormControl>
                                <div className="relative">
                                    <Shield className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                                    <Input type="password" {...field} className="pl-9 bg-background/50" />
                                </div>
                            </FormControl>
                            <FormMessage />
                        </FormItem>
                    )}
                />
            </div>

            <FormField
                control={form.control}
                name="from_address"
                render={({ field }) => (
                    <FormItem>
                        <FormLabel><Trans>From Address</Trans></FormLabel>
                        <FormControl>
                            <div className="relative">
                                <Mail className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                                <Input placeholder="notifier@example.com" {...field} className="pl-9 bg-background/50" />
                            </div>
                        </FormControl>
                        <FormMessage />
                    </FormItem>
                )}
            />

            <FormField
                control={form.control}
                name="to_addresses"
                render={({ field }) => (
                    <FormItem>
                        <FormLabel><Trans>To Addresses</Trans></FormLabel>
                        <FormControl>
                            <TagInput
                                {...field}
                                value={field.value || []}
                                onChange={field.onChange}
                                placeholder="Add email and press Enter"
                                className="bg-background/50"
                            />
                        </FormControl>
                        <FormDescription><Trans>Press Enter to add recipient</Trans></FormDescription>
                        <FormMessage />
                    </FormItem>
                )}
            />

            <FormField
                control={form.control}
                name="use_tls"
                render={({ field }) => (
                    <FormItem className="flex flex-row items-center justify-between rounded-lg border border-primary/10 bg-background/50 p-3 shadow-sm">
                        <div className="space-y-0.5">
                            <FormLabel><Trans>Use TLS/StartTLS</Trans></FormLabel>
                            <FormDescription>
                                <Trans>Enable encryption for secure communication</Trans>
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
    );
}
