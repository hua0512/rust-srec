import { UseFormReturn } from 'react-hook-form';
import { z } from 'zod';
import { Trans } from '@lingui/react/macro';
import { Shield, Globe, Lock, User } from 'lucide-react';
import {
    FormControl,
    FormDescription,
    FormItem,
    FormLabel,
} from '../../../ui/form';
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from '../../../ui/select';
import { Input } from '../../../ui/input';
import { Separator } from '../../../ui/separator';
import { UpdateTemplateRequestSchema, ProxyConfigObjectSchema } from '../../../../api/schemas';

type EditTemplateFormValues = z.infer<typeof UpdateTemplateRequestSchema>;
type ProxyConfig = z.infer<typeof ProxyConfigObjectSchema>;

interface ProxyTabProps {
    form: UseFormReturn<EditTemplateFormValues>;
}

export function ProxyTab({ form }: ProxyTabProps) {
    const rawProxyConfig = form.watch("proxy_config");

    // Helper to parse existing proxy config
    const parseProxyConfig = (jsonString: string | null | undefined): ProxyConfig | null => {
        if (!jsonString) return null;
        try {
            return JSON.parse(jsonString);
        } catch (e) {
            console.error("Failed to parse proxy config:", e);
            return null;
        }
    };

    const currentProxyConfig = parseProxyConfig(rawProxyConfig);
    const proxyMode = currentProxyConfig === null ? "default" : "custom";

    const handleModeChange = (mode: string) => {
        if (mode === "default") {
            form.setValue("proxy_config", null, { shouldDirty: true });
        } else {
            // Initialize with default custom settings
            const defaultConfig: ProxyConfig = {
                enabled: true,
                url: "",
                use_system_proxy: false
            };
            form.setValue("proxy_config", JSON.stringify(defaultConfig), { shouldDirty: true });
        }
    };

    const updateProxyField = <K extends keyof ProxyConfig>(field: K, value: ProxyConfig[K]) => {
        const newConfig = { ...currentProxyConfig, [field]: value } as ProxyConfig;
        form.setValue("proxy_config", JSON.stringify(newConfig), { shouldDirty: true });
    };

    return (
        <div className="space-y-6">
            <FormItem>
                <FormLabel className="text-base font-semibold"><Trans>Proxy Strategy</Trans></FormLabel>
                <Select
                    value={proxyMode}
                    onValueChange={handleModeChange}
                >
                    <FormControl>
                        <SelectTrigger className="w-full sm:w-[300px]">
                            <SelectValue placeholder="Select proxy strategy" />
                        </SelectTrigger>
                    </FormControl>
                    <SelectContent>
                        <SelectItem value="default">
                            <span className="font-medium"><Trans>Global Default</Trans></span>
                            <span className="ml-2 text-muted-foreground text-xs"><Trans>(Use global settings)</Trans></span>
                        </SelectItem>
                        <SelectItem value="custom">
                            <span className="font-medium"><Trans>Custom Proxy</Trans></span>
                            <span className="ml-2 text-muted-foreground text-xs"><Trans>(Override for this template)</Trans></span>
                        </SelectItem>
                    </SelectContent>
                </Select>
                <FormDescription>
                    <Trans>Choose whether to use the global proxy configuration or define a specific one for this template.</Trans>
                </FormDescription>
            </FormItem>

            {proxyMode === "custom" && currentProxyConfig && (
                <div className="space-y-4 rounded-xl border p-4 bg-muted/10 animate-in fade-in slide-in-from-top-2">
                    <div className="flex items-center gap-2 mb-2">
                        <Shield className="w-4 h-4 text-primary" />
                        <h4 className="font-medium"><Trans>Custom Proxy Configuration</Trans></h4>
                    </div>
                    <Separator />

                    <div className="space-y-4">
                        <FormItem className="flex flex-row items-center justify-between rounded-lg border p-3 bg-card">
                            <div className="space-y-0.5">
                                <FormLabel className="text-sm font-medium"><Trans>Enable Proxy</Trans></FormLabel>
                            </div>
                            <FormControl>
                                <Select
                                    value={currentProxyConfig.enabled ? "true" : "false"}
                                    onValueChange={(v) => updateProxyField("enabled", v === "true")}
                                >
                                    <SelectTrigger className="w-[100px] h-8">
                                        <SelectValue />
                                    </SelectTrigger>
                                    <SelectContent>
                                        <SelectItem value="true">On</SelectItem>
                                        <SelectItem value="false">Off</SelectItem>
                                    </SelectContent>
                                </Select>
                            </FormControl>
                        </FormItem>

                        <div className="space-y-2">
                            <FormLabel className="text-xs font-semibold uppercase text-muted-foreground flex items-center gap-1.5">
                                <Globe className="w-3 h-3" /> <Trans>URL</Trans>
                            </FormLabel>
                            <Input
                                value={currentProxyConfig.url || ""}
                                onChange={(e) => updateProxyField("url", e.target.value)}
                                placeholder="http://127.0.0.1:7890"
                                className="font-mono text-sm"
                            />
                        </div>

                        <div className="grid grid-cols-2 gap-4">
                            <div className="space-y-2">
                                <FormLabel className="text-xs font-semibold uppercase text-muted-foreground flex items-center gap-1.5">
                                    <User className="w-3 h-3" /> <Trans>Username</Trans>
                                </FormLabel>
                                <Input
                                    value={currentProxyConfig.username || ""}
                                    onChange={(e) => updateProxyField("username", e.target.value)}
                                    placeholder="Optional"
                                />
                            </div>
                            <div className="space-y-2">
                                <FormLabel className="text-xs font-semibold uppercase text-muted-foreground flex items-center gap-1.5">
                                    <Lock className="w-3 h-3" /> <Trans>Password</Trans>
                                </FormLabel>
                                <Input
                                    type="password"
                                    value={currentProxyConfig.password || ""}
                                    onChange={(e) => updateProxyField("password", e.target.value)}
                                    placeholder="Optional"
                                />
                            </div>
                        </div>
                    </div>
                </div>
            )}
        </div>
    );
}
