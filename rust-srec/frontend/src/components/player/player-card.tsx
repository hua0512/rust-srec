import { useEffect, useRef, useState } from 'react';
import Artplayer from 'artplayer';
import Hls from 'hls.js';
import mpegts from 'mpegts.js';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { AlertCircle, Loader2, RefreshCcw, Settings2, X } from 'lucide-react';
import { cn } from '@/lib/utils';
import { resolveUrl } from '@/server/functions/parse';
import { toast } from 'sonner';

export interface PlayerCardProps {
    url: string;
    title?: string;
    headers?: Record<string, string>;
    streamData?: any; // The original stream object for resolution
    onRemove?: () => void;
    className?: string;
    settingsContent?: React.ReactNode;
}

export function PlayerCard({
    url,
    title,
    headers,
    streamData,
    onRemove,
    className,
    settingsContent,
}: PlayerCardProps) {
    const artRef = useRef<HTMLDivElement>(null);
    const playerRef = useRef<Artplayer | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [loading, setLoading] = useState(true);
    const [resolving, setResolving] = useState(false);
    const [currentUrl, setCurrentUrl] = useState(url);
    const [currentHeaders, setCurrentHeaders] = useState(headers);
    const [refreshKey, setRefreshKey] = useState(0);

    const handleRefresh = () => {
        setRefreshKey((prev) => prev + 1);
        setLoading(true);
    };

    // Auto-resolve URL on mount or refresh if streamData is present
    useEffect(() => {
        const resolve = async () => {
            if (!streamData || !title) {
                setCurrentUrl(url);
                setCurrentHeaders(headers);
                return;
            }

            setResolving(true);
            try {
                const response = await resolveUrl({
                    data: {
                        url: title,
                        stream_info: streamData,
                    }
                });

                if (response.success && response.stream_info) {
                    console.log('[PlayerCard] Resolved URL:', response.stream_info.url);
                    setCurrentUrl(response.stream_info.url || url);
                    // Merge headers if any new ones are returned (though backend usually updates headers in stream_info)
                    // Ensuring we use headers from resolved stream info if available
                    setCurrentHeaders(response.stream_info.headers || headers);
                } else {
                    console.warn('[PlayerCard] URL resolution failed, using original:', response.error);
                    toast.error(`Resolution failed: ${response.error || 'Unknown error'}`);
                    setCurrentUrl(url);
                    setCurrentHeaders(headers);
                }
            } catch (err) {
                console.error('[PlayerCard] Resolution error:', err);
                setCurrentUrl(url);
                setCurrentHeaders(headers);
            } finally {
                setResolving(false);
            }
        };

        resolve();
    }, [url, headers, streamData, title, refreshKey]);

    useEffect(() => {
        if (!artRef.current || resolving) return;

        const isHLS = currentUrl.includes('.m3u8') || currentUrl.includes('m3u8');
        const isMPEGTS = currentUrl.includes('.flv') || currentUrl.includes('.ts');

        // Build proxy URL if headers are needed
        const playUrl = currentHeaders
            ? `/stream-proxy?url=${encodeURIComponent(currentUrl)}&headers=${encodeURIComponent(JSON.stringify(currentHeaders))}`
            : currentUrl;

        console.log('[PlayerCard] Init:', { url: currentUrl, headers: currentHeaders, playUrl, isHeaderPresent: !!currentHeaders });

        const options: any = {
            container: artRef.current,
            url: playUrl,
            autoplay: true,
            volume: 0.5,
            muted: false,
            autoSize: false,
            pip: true,
            mutex: false,
            setting: true,
            playbackRate: true,
            aspectRatio: true,
            fullscreen: true,
            fullscreenWeb: true,
            miniProgressBar: true,
            theme: '#3b82f6',
            type: isHLS ? 'm3u8' : isMPEGTS ? 'flv' : 'auto',
        };

        // Custom type for HLS
        if (isHLS && Hls.isSupported()) {
            console.log('[PlayerCard] Using custom type for HLS');
            options.customType = {
                m3u8: (video: HTMLMediaElement, url: string) => {
                    const hls = new Hls({
                        enableWorker: true,
                        lowLatencyMode: true,
                    });
                    hls.loadSource(url);
                    hls.attachMedia(video);
                    hls.on(Hls.Events.ERROR, (_event, data) => {
                        if (data.fatal) {
                            setError(`HLS Error: ${data.type} - ${data.details}`);
                            setLoading(false);
                        }
                    });
                },
            };
        }

        // Custom type for MPEG-TS
        if (isMPEGTS && mpegts.isSupported()) {
            console.log('[PlayerCard] Using custom type for MPEG-TS');
            options.customType = {
                flv: (video: HTMLMediaElement, url: string) => {
                    const player = mpegts.createPlayer({
                        type: 'flv',
                        url: url,
                        isLive: true,
                        cors: true
                    });
                    player.attachMediaElement(video);
                    player.load();
                    player.on(mpegts.Events.ERROR, (type, details, data) => {
                        console.error('MPEG-TS Error:', { type, details, data });
                        setError(`MPEG-TS Error: ${type} - ${details}`);
                        setLoading(false);
                    });
                },
            };
        }

        try {
            const art = new Artplayer(options);
            playerRef.current = art;

            art.on('ready', () => {
                setLoading(false);
                setError(null);
            });

            art.on('error', (err) => {
                console.log("Player Error: ", err);
                setError(`Player Error: ${err?.message || 'Unknown error'}`);
                setLoading(false);
            });
        } catch (err) {
            setError(`Failed to initialize player: ${err instanceof Error ? err.message : 'Unknown error'}`);
            setLoading(false);
        }

        return () => {
            if (playerRef.current) {
                playerRef.current.destroy(false);
                playerRef.current = null;
            }
        };
    }, [url, headers, refreshKey]);

    return (
        <Card
            className={cn(
                'relative h-full flex flex-col transition-all duration-500 hover:shadow-2xl hover:shadow-primary/10 group overflow-hidden bg-gradient-to-br from-background/80 to-background/40 backdrop-blur-xl border-border/40 hover:border-primary/20',
                className,
            )}
        >
            <div className="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent via-primary/40 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700" />

            {/* Hover Glow Effect */}
            <div className="absolute -inset-0.5 bg-gradient-to-br from-primary/5 to-transparent opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-500 pointer-events-none" />

            <CardHeader className="relative flex flex-row items-center justify-between gap-4 pb-2 space-y-0 z-10">
                <div className="flex items-center gap-3 min-w-0">
                    <div className="p-2 rounded-xl bg-primary/10 ring-1 ring-inset ring-primary/20 transition-transform duration-500 group-hover:scale-110">
                        <div className="h-4 w-4 bg-primary rounded-full animate-pulse" />
                    </div>
                    <div className="flex flex-col min-w-0">
                        <CardTitle className="text-sm font-medium truncate tracking-tight text-foreground/90 group-hover:text-primary transition-colors duration-300">
                            {title || 'Video Player'}
                        </CardTitle>
                    </div>
                </div>

                <div className="flex items-center gap-2">
                    {loading && (
                        <Badge variant="secondary" className="gap-1 bg-background/50 backdrop-blur">
                            <Loader2 className="h-3 w-3 animate-spin" />
                            Loading
                        </Badge>
                    )}
                    <Button
                        variant="ghost"
                        size="icon"
                        className="h-8 w-8 text-muted-foreground/60 hover:text-primary hover:bg-primary/10 transition-colors rounded-full"
                        onClick={handleRefresh}
                        title="Reload Player"
                    >
                        <RefreshCcw className="h-4 w-4" />
                    </Button>
                    {settingsContent && (
                        <Popover>
                            <PopoverTrigger asChild>
                                <Button
                                    variant="ghost"
                                    size="icon"
                                    className="h-8 w-8 text-muted-foreground/60 hover:text-primary hover:bg-primary/10 transition-colors rounded-full"
                                >
                                    <Settings2 className="h-4 w-4" />
                                </Button>
                            </PopoverTrigger>
                            <PopoverContent align="end" className="w-[320px] p-4 backdrop-blur-xl bg-background/80 border-border/40 text-foreground z-[200]">
                                {settingsContent}
                            </PopoverContent>
                        </Popover>
                    )}
                    {onRemove && (
                        <Button
                            variant="ghost"
                            size="icon"
                            className="h-8 w-8 text-muted-foreground/40 hover:text-destructive hover:bg-destructive/10 transition-colors rounded-full"
                            onClick={onRemove}
                        >
                            <X className="h-4 w-4" />
                        </Button>
                    )}
                </div>
            </CardHeader>

            <CardContent className="relative p-0 flex-1 min-h-[500px] bg-black/50 group-hover:bg-black/40 transition-colors rounded-b-xl overflow-hidden">
                {error ? (
                    <div className="flex flex-col items-center justify-center h-full text-center space-y-3 p-8">
                        <div className="p-3 rounded-full bg-destructive/10 text-destructive mb-2">
                            <AlertCircle className="h-8 w-8" />
                        </div>
                        <p className="text-sm font-medium text-destructive">Playback Error</p>
                        <p className="text-xs text-muted-foreground max-w-[250px]">{error}</p>
                    </div>
                ) : (
                    <div ref={artRef} className="w-full h-full absolute inset-0" />
                )}
            </CardContent>
        </Card>
    );
}
