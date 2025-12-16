import { Link } from '@tanstack/react-router';
import {
    Card,
    CardContent,
    CardHeader,
    CardTitle,
} from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Skeleton } from '@/components/ui/skeleton';
import { Trans } from '@lingui/react/macro';
import { format } from 'date-fns';
import { motion, AnimatePresence } from 'motion/react';
import { cn } from '@/lib/utils';
import { formatDuration } from '@/lib/format';
import {
    Server,
    Clock,
} from 'lucide-react';

interface JobsTabProps {
    isLoading: boolean;
    jobs: any[];
}

export function JobsTab({ isLoading, jobs }: JobsTabProps) {
    return (
        <motion.div
            initial={{ opacity: 0, y: 10 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ delay: 0.2 }}
        >
            <Card className="bg-card/40 backdrop-blur-sm border-border/40 shadow-sm">
                <CardHeader className="border-b border-border/40 pb-4">
                    <CardTitle className="text-lg font-semibold flex items-center gap-2">
                        <Server className="h-5 w-5 text-primary/70" />
                        <Trans>Pipeline Jobs</Trans>
                    </CardTitle>
                </CardHeader>
                <CardContent className="p-0">
                    {isLoading ? (
                        <div className="p-6 space-y-4">
                            <Skeleton className="h-12 w-full" />
                        </div>
                    ) : jobs.length === 0 ? (
                        <div className="p-10 text-center text-muted-foreground">
                            <Server className="h-10 w-10 mx-auto mb-3 opacity-20" />
                            <p>No pipeline jobs found.</p>
                        </div>
                    ) : (
                        <div className="divide-y divide-border/40">
                            <AnimatePresence mode='popLayout'>
                                {jobs.map((job: any, index: number) => (
                                    <motion.div
                                        key={job.id}
                                        initial={{ opacity: 0, x: -10 }}
                                        animate={{ opacity: 1, x: 0 }}
                                        transition={{ delay: index * 0.05 }}
                                    >
                                        <Link
                                            to="/pipeline/jobs/$jobId"
                                            params={{ jobId: job.id }}
                                            className="flex items-center justify-between p-4 hover:bg-muted/30 transition-colors group"
                                        >
                                            <div className="flex items-center gap-4">
                                                <Badge
                                                    variant={
                                                        job.status === 'COMPLETED'
                                                            ? 'default'
                                                            : job.status === 'FAILED'
                                                                ? 'destructive'
                                                                : 'secondary'
                                                    }
                                                    className={cn(
                                                        'w-24 justify-center',
                                                        job.status === 'COMPLETED' &&
                                                        'bg-green-500/15 text-green-600 hover:bg-green-500/25 border-green-500/20',
                                                        job.status === 'PROCESSING' &&
                                                        'bg-blue-500/15 text-blue-600 hover:bg-blue-500/25 border-blue-500/20 animate-pulse',
                                                    )}
                                                >
                                                    {job.status}
                                                </Badge>
                                                <div>
                                                    <p className="font-medium text-sm group-hover:text-primary transition-colors">
                                                        {job.processor_type}
                                                        {job.execution_info?.current_step && job.execution_info?.total_steps && (
                                                            <span className="text-xs text-muted-foreground font-normal ml-2">
                                                                Step {job.execution_info.current_step}/
                                                                {job.execution_info.total_steps}
                                                            </span>
                                                        )}
                                                    </p>
                                                    <p className="text-xs text-muted-foreground font-mono mt-0.5">
                                                        ID: {job.id}
                                                    </p>
                                                </div>
                                            </div>
                                            <div className="flex items-center gap-6 text-sm text-muted-foreground">
                                                <div className="flex items-center gap-1.5">
                                                    <Clock className="h-3.5 w-3.5" />
                                                    {format(new Date(job.created_at), 'MMM d, HH:mm:ss')}
                                                </div>
                                                {job.duration_secs && (
                                                    <div className="font-mono">
                                                        {formatDuration(job.duration_secs)}
                                                    </div>
                                                )}
                                            </div>
                                        </Link>
                                    </motion.div>
                                ))}
                            </AnimatePresence>
                        </div>
                    )}
                </CardContent>
            </Card>
        </motion.div>
    );
}
