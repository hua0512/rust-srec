import { Card, CardContent, CardHeader, CardTitle } from '../../ui/card';
import { Button } from '../../ui/button';
import { Trans } from '@lingui/react/macro';
import { Tv, Cookie, Settings } from 'lucide-react';
import { PlatformConfigSchema } from '../../../api/schemas';
import { z } from 'zod';
import { EditPlatformDialog } from './edit-platform-dialog';

interface PlatformCardProps {
    platform: z.infer<typeof PlatformConfigSchema>;
}

export function PlatformCard({ platform }: PlatformCardProps) {
    return (
        <Card className="group overflow-hidden hover:shadow-lg transition-all duration-300 border-muted/60 flex flex-col">
            <CardHeader className="pb-3 border-b bg-muted/20">
                <div className="flex items-center justify-between">
                    <CardTitle className="flex items-center gap-2.5 text-lg">
                        <div className="w-2.5 h-2.5 rounded-full bg-primary/60 group-hover:bg-primary transition-colors ring-2 ring-primary/20" />
                        {platform.name}
                    </CardTitle>
                    <div className="flex gap-1">
                        {platform.record_danmu && (
                            <div className="p-1.5 rounded-md bg-green-500/10 text-green-600 dark:text-green-400" title="Danmu Recording Enabled">
                                <Tv className="w-3.5 h-3.5" />
                            </div>
                        )}
                        {platform.cookies && (
                            <div className="p-1.5 rounded-md bg-orange-500/10 text-orange-600 dark:text-orange-400" title="Cookies Set">
                                <Cookie className="w-3.5 h-3.5" />
                            </div>
                        )}
                    </div>
                </div>
            </CardHeader>
            <CardContent className="pt-4 flex-1 grid gap-4">
                <div className="grid grid-cols-2 gap-4 text-sm">
                    <div className="space-y-1">
                        <span className="text-muted-foreground text-xs uppercase tracking-wider font-semibold"><Trans>Fetch</Trans></span>
                        <div className="font-mono bg-muted/50 p-1.5 rounded text-center">
                            {platform.fetch_delay_ms ? `${(platform.fetch_delay_ms / 1000).toFixed(0)}s` : <span className="text-muted-foreground">-</span>}
                        </div>
                    </div>
                    <div className="space-y-1">
                        <span className="text-muted-foreground text-xs uppercase tracking-wider font-semibold"><Trans>Download</Trans></span>
                        <div className="font-mono bg-muted/50 p-1.5 rounded text-center">
                            {platform.download_delay_ms ? `${(platform.download_delay_ms / 1000).toFixed(0)}s` : <span className="text-muted-foreground">-</span>}
                        </div>
                    </div>
                </div>

                <div className="mt-auto pt-2">
                    <EditPlatformDialog
                        platform={platform}
                        trigger={
                            <Button variant="outline" className="w-full group-hover:border-primary/50 group-hover:text-primary transition-colors">
                                <Settings className="w-4 h-4 mr-2" />
                                <Trans>Configure</Trans>
                            </Button>
                        }
                    />
                </div>
            </CardContent>
        </Card>
    );
}
