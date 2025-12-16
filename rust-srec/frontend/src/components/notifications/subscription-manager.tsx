import { useState } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import {
    getSubscriptions,
    updateSubscriptions,
    listEventTypes,
} from '@/server/functions/notifications';
import {
    Dialog,
    DialogContent,
    DialogHeader,
    DialogTitle,
    DialogDescription,
    DialogFooter,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Checkbox } from '@/components/ui/checkbox';
import { Label } from '@/components/ui/label';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Skeleton } from '@/components/ui/skeleton';
import { Badge } from '@/components/ui/badge';
import { toast } from 'sonner';
import { NotificationChannel } from '@/api/schemas';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import {
    Loader2, BellRing, Check, Search, Filter,
    Radio, Download, CheckCircle2, XCircle, FileVideo, FileCheck,
    Ban, AlertOctagon, Workflow, CheckSquare, XSquare, MinusCircle,
    ZapOff, HardDrive, AlertTriangle, Settings
} from 'lucide-react';
import { Input } from '@/components/ui/input';
import { motion } from 'motion/react';


interface SubscriptionManagerProps {
    channel: NotificationChannel | null;
    open: boolean;
    onOpenChange: (open: boolean) => void;
}

const getEventIcon = (eventType: string) => {
    switch (eventType) {
        // Stream
        case 'stream_online':
            return { icon: Radio, color: 'text-green-500', bg: 'bg-green-500/10' };
        case 'stream_offline':
            return { icon: Radio, color: 'text-slate-400', bg: 'bg-slate-500/10' };

        // Download
        case 'download_started':
            return { icon: Download, color: 'text-blue-500', bg: 'bg-blue-500/10' };
        case 'download_completed':
            return { icon: CheckCircle2, color: 'text-green-500', bg: 'bg-green-500/10' };
        case 'download_error':
            return { icon: XCircle, color: 'text-red-500', bg: 'bg-red-500/10' };
        case 'segment_started':
            return { icon: FileVideo, color: 'text-indigo-400', bg: 'bg-indigo-500/10' };
        case 'segment_completed':
            return { icon: FileCheck, color: 'text-indigo-500', bg: 'bg-indigo-500/10' };
        case 'download_cancelled':
            return { icon: Ban, color: 'text-orange-500', bg: 'bg-orange-500/10' };
        case 'download_rejected':
            return { icon: AlertOctagon, color: 'text-red-500', bg: 'bg-red-500/10' };

        // Pipeline
        case 'pipeline_started':
            return { icon: Workflow, color: 'text-blue-500', bg: 'bg-blue-500/10' };
        case 'pipeline_completed':
            return { icon: CheckSquare, color: 'text-green-500', bg: 'bg-green-500/10' };
        case 'pipeline_failed':
            return { icon: XSquare, color: 'text-red-500', bg: 'bg-red-500/10' };
        case 'pipeline_cancelled':
            return { icon: MinusCircle, color: 'text-orange-500', bg: 'bg-orange-500/10' };

        // System
        case 'fatal_error':
            return { icon: ZapOff, color: 'text-red-600', bg: 'bg-red-600/10' };
        case 'out_of_space':
            return { icon: HardDrive, color: 'text-red-600', bg: 'bg-red-600/10' };
        case 'pipeline_queue_warning':
            return { icon: AlertTriangle, color: 'text-yellow-500', bg: 'bg-yellow-500/10' };
        case 'config_updated':
            return { icon: Settings, color: 'text-gray-500', bg: 'bg-gray-500/10' };

        default:
            return { icon: BellRing, color: 'text-gray-400', bg: 'bg-gray-500/10' };
    }
};

const getEventDescription = (eventType: string) => {
    switch (eventType) {
        case 'stream_online':
            return t`Receive a notification when a streamer goes live.`;
        case 'stream_offline':
            return t`Receive a notification when a stream ends.`;
        case 'download_started':
            return t`Triggered when a new download recording begins.`;
        case 'download_completed':
            return t`Triggered when a download successfully completes.`;
        case 'download_error':
            return t`Triggered when a download fails with an error.`;
        case 'segment_started':
            return t`Triggered when a new file segment is created.`;
        case 'segment_completed':
            return t`Triggered when a file segment is finished.`;
        case 'download_cancelled':
            return t`Triggered when a download is manually cancelled.`;
        case 'download_rejected':
            return t`Triggered when a download is rejected (e.g., circuit breaker).`;
        case 'config_updated':
            return t`Triggered when streamer configuration is dynamically updated.`;
        case 'pipeline_started':
            return t`Triggered when a post-processing pipeline job starts.`;
        case 'pipeline_completed':
            return t`Triggered when a pipeline job finishes successfully.`;
        case 'pipeline_failed':
            return t`Triggered when a pipeline job fails.`;
        case 'pipeline_cancelled':
            return t`Triggered when a pipeline job is cancelled.`;
        case 'fatal_error':
            return t`Critical system errors or streamer failures.`;
        case 'out_of_space':
            return t`Alerts when disk space is running low.`;
        case 'pipeline_queue_warning':
            return t`Warning when the processing queue gets too long.`;
        case 'pipeline_queue_critical':
            return t`Critical alert when the processing queue is full.`;
        case 'system_startup':
            return t`Triggered when the application starts up.`;
        case 'system_shutdown':
            return t`Triggered when the application shuts down.`;
        default:
            return '';
    }
};

export function SubscriptionManager({
    channel,
    open,
    onOpenChange,
}: SubscriptionManagerProps) {
    const queryClient = useQueryClient();
    const [selectedEvents, setSelectedEvents] = useState<string[]>([]);
    const [searchQuery, setSearchQuery] = useState('');

    // Fetch event types (available options)
    const { data: eventTypes, isLoading: isLoadingTypes } = useQuery({
        queryKey: ['notification-event-types'],
        queryFn: () => listEventTypes(),
        staleTime: Infinity,
    });

    // Fetch current subscriptions for the channel
    const { data: currentSubs, isLoading: isLoadingSubs } = useQuery({
        queryKey: ['subscriptions', channel?.id],
        queryFn: () => getSubscriptions({ data: channel!.id }),
        enabled: !!channel && open,
    });

    // Sync state when data loads
    useQuery({
        queryKey: ['sync-subs', channel?.id, currentSubs],
        queryFn: async () => {
            if (currentSubs) {
                setSelectedEvents(currentSubs);
            }
            return null;
        },
        enabled: !!currentSubs,
    });

    const mutation = useMutation({
        mutationFn: (events: string[]) =>
            updateSubscriptions({ data: { id: channel!.id, events } }),
        onSuccess: () => {
            toast.success(t`Subscriptions updated`);
            queryClient.invalidateQueries({ queryKey: ['subscriptions', channel?.id] });
            onOpenChange(false);
        },
        onError: (err: any) => {
            toast.error(err.message || t`Failed to update subscriptions`);
        },
    });

    const handleToggle = (eventType: string) => {
        setSelectedEvents((prev) =>
            prev.includes(eventType)
                ? prev.filter((e) => e !== eventType)
                : [...prev, eventType],
        );
    };

    // Animation variants
    const container = {
        hidden: { opacity: 0 },
        show: {
            opacity: 1,
            transition: {
                staggerChildren: 0.05
            }
        }
    };

    const item = {
        hidden: { opacity: 0, x: -10 },
        show: { opacity: 1, x: 0 }
    };

    const handleSelectAll = (filteredTypes: typeof eventTypes) => {
        if (!filteredTypes) return;
        const allSelected = filteredTypes.every(t => selectedEvents.includes(t.event_type));

        if (allSelected) {
            // Deselect visible
            const toRemove = filteredTypes.map(t => t.event_type);
            setSelectedEvents(prev => prev.filter(e => !toRemove.includes(e)));
        } else {
            // Select visible
            const toAdd = filteredTypes.map(t => t.event_type);
            setSelectedEvents(prev => [...new Set([...prev, ...toAdd])]);
        }
    };

    const handleSave = () => {
        if (channel) {
            mutation.mutate(selectedEvents);
        }
    };

    if (!channel) return null;

    const isLoading = isLoadingTypes || isLoadingSubs;

    const filteredEventTypes = eventTypes?.filter(type =>
        type.event_type.toLowerCase().includes(searchQuery.toLowerCase()) ||
        type.label.toLowerCase().includes(searchQuery.toLowerCase())
    );

    const isAllSelected = filteredEventTypes && filteredEventTypes.length > 0 && filteredEventTypes.every(t => selectedEvents.includes(t.event_type));

    return (
        <Dialog open={open} onOpenChange={onOpenChange}>
            <DialogContent className="sm:max-w-[550px] max-h-[85vh] bg-background/95 backdrop-blur-xl border-border/50 shadow-2xl">
                <DialogHeader className="pb-4 border-b border-border/40 space-y-2">
                    <DialogTitle className="flex items-center gap-2 text-xl">
                        <BellRing className="h-5 w-5 text-primary" />
                        <Trans>Manage Subscriptions</Trans>
                    </DialogTitle>
                    <DialogDescription>
                        <Trans>
                            Select the events that should trigger notifications for{' '}
                            <span className="font-semibold text-foreground">{channel.name}</span>.
                        </Trans>
                    </DialogDescription>
                </DialogHeader>

                {isLoading ? (
                    <div className="space-y-4 py-6 px-1">
                        <div className="space-y-2">
                            <Skeleton className="h-10 w-full rounded-lg" />
                        </div>
                        <div className="space-y-3">
                            <Skeleton className="h-16 w-full rounded-xl" />
                            <Skeleton className="h-16 w-full rounded-xl" />
                            <Skeleton className="h-16 w-full rounded-xl" />
                        </div>
                    </div>
                ) : (
                    <div className="flex flex-col gap-4 py-4">
                        {/* Search and Filters */}
                        <div className="flex items-center gap-2">
                            <div className="relative flex-1">
                                <Search className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
                                <Input
                                    placeholder={t`Search events...`}
                                    value={searchQuery}
                                    onChange={(e) => setSearchQuery(e.target.value)}
                                    className="pl-9 bg-muted/40 border-primary/10 transition-colors focus:bg-background h-9"
                                />
                            </div>
                            <Button
                                variant={isAllSelected ? "secondary" : "outline"}
                                size="sm"
                                onClick={() => handleSelectAll(filteredEventTypes)}
                                className="h-9 whitespace-nowrap text-xs"
                            >
                                {isAllSelected ? (
                                    <>
                                        <Check className="mr-1 h-3.5 w-3.5" />
                                        <Trans>Deselect All</Trans>
                                    </>
                                ) : (
                                    <Trans>Select All</Trans>
                                )}
                            </Button>
                        </div>

                        <ScrollArea className="h-[400px] -mr-4 pr-4">
                            <motion.div
                                className="space-y-2"
                                variants={container}
                                initial="hidden"
                                animate="show"
                            >
                                {filteredEventTypes?.length === 0 ? (
                                    <div className="flex flex-col items-center justify-center py-12 text-center text-muted-foreground">
                                        <Filter className="h-8 w-8 mb-2 opacity-20" />
                                        <p className="text-sm"><Trans>No matching events found</Trans></p>
                                    </div>
                                ) : (
                                    filteredEventTypes?.map((type) => {
                                        const isSelected = selectedEvents.includes(type.event_type);
                                        const iconConfig = getEventIcon(type.event_type);
                                        const IconComponent = iconConfig.icon;
                                        return (
                                            <motion.div
                                                key={type.event_type}
                                                variants={item}
                                                className={`
                                                    group flex flex-row items-center space-x-3 rounded-xl border p-3 transition-all duration-200 cursor-pointer
                                                    ${isSelected
                                                        ? 'bg-primary/5 border-primary/20 shadow-[0_0_15px_-3px_rgba(0,0,0,0.1)] dark:shadow-[0_0_15px_-3px_rgba(255,255,255,0.05)]'
                                                        : 'bg-card/40 border-border/40 hover:bg-muted/40 hover:border-primary/10'
                                                    }
                                                `}
                                                onClick={() => handleToggle(type.event_type)}
                                            >
                                                <Checkbox
                                                    checked={isSelected}
                                                    onCheckedChange={() => handleToggle(type.event_type)}
                                                    className={`transition-all duration-200 ${isSelected ? 'data-[state=checked]:bg-primary data-[state=checked]:border-primary' : ''}`}
                                                />

                                                <div className={`p-2 rounded-lg ${iconConfig.bg}`}>
                                                    <IconComponent className={`h-4 w-4 ${iconConfig.color}`} />
                                                </div>

                                                <div className="flex-1 space-y-1">
                                                    <div className="flex items-center justify-between">
                                                        <Label className={`font-medium cursor-pointer transition-colors ${isSelected ? 'text-primary' : 'text-foreground'}`}>
                                                            {type.label}
                                                        </Label>
                                                        <Badge
                                                            variant="outline"
                                                            className={`text-[10px] h-5 transition-colors ${isSelected ? 'bg-primary/10 border-primary/20 text-primary' : 'text-muted-foreground'}`}
                                                        >
                                                            {type.priority}
                                                        </Badge>
                                                    </div>
                                                    <p className="text-xs text-muted-foreground/80 leading-relaxed font-mono">
                                                        {type.event_type}
                                                    </p>
                                                    <p className="text-xs text-muted-foreground/60 leading-relaxed">
                                                        {getEventDescription(type.event_type)}
                                                    </p>
                                                </div>
                                            </motion.div>
                                        );
                                    })
                                )}
                            </motion.div>
                        </ScrollArea>

                        <div className="flex items-center justify-between text-xs text-muted-foreground px-1">
                            <span>{selectedEvents.length} <Trans>selected</Trans></span>
                            <span>{filteredEventTypes?.length || 0} <Trans>available</Trans></span>
                        </div>
                    </div>
                )}

                <DialogFooter className="pt-4 border-t border-border/40">
                    <Button variant="ghost" onClick={() => onOpenChange(false)}>
                        <Trans>Cancel</Trans>
                    </Button>
                    <Button
                        onClick={handleSave}
                        disabled={mutation.isPending || isLoading}
                        className="bg-primary hover:bg-primary/90 shadow-lg shadow-primary/20"
                    >
                        {mutation.isPending && (
                            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                        )}
                        <Trans>Save Changes</Trans>
                    </Button>
                </DialogFooter>
            </DialogContent>
        </Dialog>
    );
}
