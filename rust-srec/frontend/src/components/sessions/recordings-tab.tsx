import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Skeleton } from '@/components/ui/skeleton';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { motion, AnimatePresence } from 'motion/react';
import { formatBytes } from '@/lib/format';
import { FileVideo, Download, Play, Video, MessageSquare } from 'lucide-react';
import { isPlayable } from '@/lib/media';

interface RecordingsTabProps {
  isLoading: boolean;
  outputs: any[];
  onDownload: (id: string, name: string) => void;
  onPlay: (output: any) => void;
}

export function RecordingsTab({
  isLoading,
  outputs,
  onDownload,
  onPlay,
}: RecordingsTabProps) {
  const { i18n } = useLingui();

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: 0.2 }}
    >
      <Card className="bg-card/40 backdrop-blur-sm border-border/40 shadow-sm">
        <CardHeader className="border-b border-border/40 pb-4 flex flex-row items-center justify-between">
          <CardTitle className="text-lg font-semibold flex items-center gap-2">
            <FileVideo className="h-5 w-5 text-primary/70" />
            <Trans>Media Outputs</Trans>
          </CardTitle>
          <Badge variant="secondary" className="font-mono text-xs">
            <Trans>{outputs.length} Files</Trans>
          </Badge>
        </CardHeader>
        <CardContent className="p-0">
          {isLoading ? (
            <div className="p-6 space-y-4">
              <Skeleton className="h-12 w-full" />
              <Skeleton className="h-12 w-full" />
            </div>
          ) : outputs.length === 0 ? (
            <div className="p-10 text-center text-muted-foreground">
              <Video className="h-10 w-10 mx-auto mb-3 opacity-20" />
              <p>
                <Trans>No media outputs generated yet.</Trans>
              </p>
            </div>
          ) : (
            <div className="divide-y divide-border/40">
              <AnimatePresence mode="popLayout">
                {outputs.map((output: any, index: number) => (
                  <motion.div
                    key={output.id}
                    initial={{ opacity: 0, x: -10 }}
                    animate={{ opacity: 1, x: 0 }}
                    transition={{ delay: index * 0.05 }}
                    className="p-4 flex flex-col sm:flex-row sm:items-center justify-between gap-4 hover:bg-muted/30 transition-colors"
                  >
                    <div className="flex items-center gap-4 overflow-hidden">
                      <div className="h-10 w-10 rounded-lg bg-primary/10 flex items-center justify-center shrink-0">
                        <FileVideo className="h-5 w-5 text-primary" />
                      </div>
                      <div className="min-w-0">
                        <p
                          className="font-medium text-sm truncate"
                          title={output.file_path}
                        >
                          {output.file_path.split('/').pop()}
                        </p>
                        <div className="flex items-center gap-3 text-xs text-muted-foreground mt-1">
                          <Badge
                            variant="outline"
                            className="text-[10px] px-1 h-4 uppercase"
                          >
                            {output.format}
                          </Badge>
                          <span>{formatBytes(output.file_size_bytes)}</span>
                          <span>•</span>
                          <span>
                            {i18n.date(new Date(output.created_at), {
                              month: 'short',
                              day: 'numeric',
                              hour: 'numeric',
                              minute: 'numeric',
                            })}
                          </span>
                        </div>
                      </div>
                    </div>
                    <div className="flex items-center gap-2 shrink-0">
                      <Button
                        variant="outline"
                        size="sm"
                        className="h-8 text-xs"
                        onClick={() =>
                          onDownload(
                            output.id,
                            output.file_path.split('/').pop() || 'video',
                          )
                        }
                      >
                        <Download className="mr-2 h-3 w-3" />{' '}
                        <Trans>Download</Trans>
                      </Button>
                      {output.format === 'DANMU_XML' && (
                        <Button
                          variant="secondary"
                          size="sm"
                          className="h-8 text-xs"
                          onClick={(e) => {
                            e.stopPropagation();
                            onPlay(output);
                          }}
                        >
                          <MessageSquare className="mr-2 h-3 w-3" />{' '}
                          <Trans>View Danmu</Trans>
                        </Button>
                      )}
                      {isPlayable(output) && (
                        <Button
                          variant="default"
                          size="sm"
                          className="h-8 text-xs"
                          onClick={(e) => {
                            e.stopPropagation();
                            onPlay(output);
                          }}
                        >
                          <Play className="mr-2 h-3 w-3" /> <Trans>Play</Trans>
                        </Button>
                      )}
                    </div>
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
