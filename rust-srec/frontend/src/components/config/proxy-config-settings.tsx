import { useState, useEffect } from 'react';
import { z } from 'zod';
import { ProxyConfigObjectSchema } from '../../api/schemas';
import { Input } from '../ui/input';
import { Label } from '../ui/label';
import { Switch } from '../ui/switch';
import { Trans } from '@lingui/react/macro';
import { Globe, Lock, User, Monitor, ShieldCheck } from 'lucide-react';
import { cn } from '../../lib/utils';

type ProxyConfig = z.infer<typeof ProxyConfigObjectSchema>;

interface ProxyConfigSettingsProps {
    value: string | null | undefined;
    onChange: (value: string | null) => void;
}

export function ProxyConfigSettings({ value, onChange }: ProxyConfigSettingsProps) {
    const [parsedConfig, setParsedConfig] = useState<ProxyConfig>({
        enabled: false,
        use_system_proxy: false,
    });

    // Parse incoming JSON string
    useEffect(() => {
        if (!value) {
            setParsedConfig({ enabled: false, use_system_proxy: false });
            return;
        }
        try {
            const parsed = JSON.parse(value);
            // safe parse with schema to ensure structure
            const result = ProxyConfigObjectSchema.safeParse(parsed);
            if (result.success) {
                setParsedConfig(result.data);
            }
        } catch (e) {
            console.error("Failed to parse proxy config JSON", e);
        }
    }, [value]);

    const handleChange = (newConfig: Partial<ProxyConfig>) => {
        const updated = { ...parsedConfig, ...newConfig };
        setParsedConfig(updated);

        // Serialize and emit
        onChange(JSON.stringify(updated));
    };

    return (
        <div className={cn(
            "rounded-xl border border-dashed transition-all duration-200",
            parsedConfig.enabled ? "bg-accent/5 border-primary/20" : "bg-muted/10 border-muted"
        )}>
            <div className="p-4 space-y-4">
                <div className="flex items-center justify-between">
                    <div className="flex items-center gap-3">
                        <div className={cn(
                            "p-2 rounded-lg transition-colors",
                            parsedConfig.enabled ? "bg-primary/10 text-primary" : "bg-muted text-muted-foreground"
                        )}>
                            <ShieldCheck className="w-5 h-5" />
                        </div>
                        <div className="space-y-0.5">
                            <Label htmlFor="proxy-enabled" className="text-base font-medium cursor-pointer">
                                <Trans>Enable Proxy</Trans>
                            </Label>
                            <p className="text-xs text-muted-foreground">
                                <Trans>Route traffic through an intermediate server</Trans>
                            </p>
                        </div>
                    </div>
                    <Switch
                        id="proxy-enabled"
                        checked={parsedConfig.enabled}
                        onCheckedChange={(checked) => handleChange({ enabled: checked })}
                    />
                </div>

                {parsedConfig.enabled && (
                    <div className="space-y-4 animate-in fade-in slide-in-from-top-2 pt-2">
                        <div className="flex items-center justify-between p-3 bg-background rounded-lg border">
                            <div className="flex items-center gap-3">
                                <Monitor className="w-4 h-4 text-muted-foreground" />
                                <Label htmlFor="system-proxy" className="text-sm font-medium cursor-pointer">
                                    <Trans>Use System Proxy</Trans>
                                </Label>
                            </div>
                            <Switch
                                id="system-proxy"
                                checked={parsedConfig.use_system_proxy}
                                onCheckedChange={(checked) => handleChange({ use_system_proxy: checked })}
                            />
                        </div>

                        {!parsedConfig.use_system_proxy && (
                            <div className="grid gap-4 p-4 rounded-lg bg-background border">
                                <div className="space-y-2">
                                    <Label htmlFor="proxy-url" className="flex items-center gap-2 text-xs font-semibold uppercase text-muted-foreground">
                                        <Globe className="w-3 h-3" />
                                        <Trans>Proxy URL</Trans>
                                    </Label>
                                    <Input
                                        id="proxy-url"
                                        placeholder="http://127.0.0.1:8080"
                                        className="font-mono"
                                        value={parsedConfig.url || ''}
                                        onChange={(e) => handleChange({ url: e.target.value })}
                                    />
                                </div>
                                <div className="grid grid-cols-2 gap-4">
                                    <div className="space-y-2">
                                        <Label htmlFor="proxy-username" className="flex items-center gap-2 text-xs font-semibold uppercase text-muted-foreground">
                                            <User className="w-3 h-3" />
                                            <Trans>Username</Trans>
                                        </Label>
                                        <Input
                                            id="proxy-username"
                                            placeholder="Optional"
                                            value={parsedConfig.username || ''}
                                            onChange={(e) => handleChange({ username: e.target.value })}
                                            autoComplete="off"
                                        />
                                    </div>
                                    <div className="space-y-2">
                                        <Label htmlFor="proxy-password" className="flex items-center gap-2 text-xs font-semibold uppercase text-muted-foreground">
                                            <Lock className="w-3 h-3" />
                                            <Trans>Password</Trans>
                                        </Label>
                                        <Input
                                            id="proxy-password"
                                            type="password"
                                            placeholder="Optional"
                                            value={parsedConfig.password || ''}
                                            onChange={(e) => handleChange({ password: e.target.value })}
                                            autoComplete="off"
                                        />
                                    </div>
                                </div>
                            </div>
                        )}
                    </div>
                )}
            </div>
        </div>
    );
}
