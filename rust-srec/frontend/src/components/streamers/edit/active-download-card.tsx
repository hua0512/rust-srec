import { Download, Zap, Save, Clock, Layers, Gauge } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { CountUp } from '@/components/ui/count-up';
import { motion, AnimatePresence } from 'motion/react';
import { Trans } from '@lingui/react/macro';
import { formatBytes, formatDuration, cn } from '@/lib/utils';
import { Download as DownloadType } from '@/store/downloads';

interface ActiveDownloadCardProps {
    downloads: DownloadType[];
    isRecording: boolean;
}

export function ActiveDownloadCard({ downloads, isRecording }: ActiveDownloadCardProps) {
    return (
        <AnimatePresence mode="popLayout">
            {isRecording && downloads.map(download => (
                <motion.div
                    key={download.downloadId}
                    initial={{ opacity: 0, y: 20, scale: 0.95 }}
                    animate={{ opacity: 1, y: 0, scale: 1 }}
                    exit={{ opacity: 0, scale: 0.95, transition: { duration: 0.2 } }}
                    layout
                    className="p-6 rounded-xl border bg-card/50 shadow-sm space-y-4 border-red-500/20 bg-red-500/5"
                >
                    <div className="flex items-center justify-between text-sm font-semibold text-red-500">
                        <div className="flex items-center gap-2">
                            <Download className="w-4 h-4 animate-bounce" /> <Trans>Active Download</Trans>
                        </div>
                        <Badge variant="outline" className="border-red-500/30 text-red-500 bg-red-500/10 animate-pulse">
                            {download.status || 'Downloading'}
                        </Badge>
                    </div>

                    <div className="grid grid-cols-2 gap-4 pt-2">
                        <div className="space-y-1">
                            <div className="text-xs text-muted-foreground flex items-center gap-1.5">
                                <Zap className="w-3 h-3" /> <Trans>Speed</Trans>
                            </div>
                            <div className="font-mono font-medium text-lg">
                                <CountUp value={Number(download.speedBytesPerSec)} formatter={(v) => formatBytes(v) + '/s'} />
                            </div>
                        </div>
                        <div className="space-y-1">
                            <div className="text-xs text-muted-foreground flex items-center gap-1.5">
                                <Save className="w-3 h-3" /> <Trans>Size</Trans>
                            </div>
                            <div className="font-mono font-medium text-lg">
                                <CountUp value={Number(download.bytesDownloaded)} formatter={(v) => formatBytes(v)} />
                            </div>
                        </div>
                        <div className="space-y-1">
                            <div className="text-xs text-muted-foreground flex items-center gap-1.5">
                                <Clock className="w-3 h-3" /> <Trans>Duration</Trans>
                            </div>
                            <div className="font-mono font-medium text-lg">
                                {formatDuration(download.durationSecs)}
                            </div>
                        </div>
                        <div className="space-y-1">
                            <div className="text-xs text-muted-foreground flex items-center gap-1.5">
                                <Layers className="w-3 h-3" /> <Trans>Segments</Trans>
                            </div>
                            <div className="font-mono font-medium text-lg">
                                <CountUp value={download.segmentsCompleted} />
                            </div>
                        </div>
                        <div className="space-y-1">
                            <div className="text-xs text-muted-foreground flex items-center gap-1.5">
                                <Gauge className="w-3 h-3" /> <Trans>Ratio</Trans>
                            </div>
                            <div className={cn(
                                "font-mono font-medium text-lg",
                                download.playbackRatio < 0.9 ? "text-yellow-500" : (download.playbackRatio > 1.1 ? "text-emerald-500" : "")
                            )}>
                                <CountUp value={download.playbackRatio * 100} formatter={(v) => (Number(v) / 100).toFixed(2) + 'x'} />
                            </div>
                        </div>
                    </div>
                </motion.div>
            ))}
        </AnimatePresence>
    );
}
