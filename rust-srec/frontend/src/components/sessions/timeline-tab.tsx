import { Card, CardContent } from '@/components/ui/card';
import { Badge } from '@/components/ui/badge';
import { useLingui } from '@lingui/react';
import { Trans } from '@lingui/react/macro';
import { motion } from 'motion/react';
import { Clock, Circle, ArrowDown } from 'lucide-react';
import { cn } from '@/lib/utils';

interface TimelineTabProps {
  session: any;
}

export function TimelineTab({ session }: TimelineTabProps) {
  const { i18n } = useLingui();
  const titles = session.titles || [];

  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: 0.2 }}
      className="max-w-4xl mx-auto py-8"
    >
      {titles.length === 0 ? (
        <div className="text-center py-20 text-muted-foreground flex flex-col items-center gap-6">
          <div className="h-24 w-24 bg-muted/30 rounded-full flex items-center justify-center border border-border/50">
            <Clock className="h-10 w-10 opacity-20" />
          </div>
          <div className="space-y-1">
            <p className="text-lg font-medium text-foreground">
              <Trans>No Title Changes</Trans>
            </p>
            <p className="text-sm">
              <Trans>
                The stream title remained constant throughout the session.
              </Trans>
            </p>
          </div>
        </div>
      ) : (
        <div className="relative">
          {/* Central Line */}
          <div className="absolute left-8 md:left-1/2 top-4 bottom-4 w-px bg-gradient-to-b from-transparent via-border/60 to-transparent -translate-x-1/2" />

          <div className="space-y-12">
            {titles.map((t: any, i: number) => {
              const isFirst = i === 0;
              const isLast = i === titles.length - 1;

              return (
                <motion.div
                  key={i}
                  initial={{ opacity: 0, y: 20 }}
                  animate={{ opacity: 1, y: 0 }}
                  transition={{ delay: i * 0.1 }}
                  className={cn(
                    'relative flex flex-col md:flex-row gap-8 md:gap-0 items-start md:items-center group',
                    i % 2 === 0 ? 'md:flex-row-reverse' : '',
                  )}
                >
                  {/* Timeline Node */}
                  <div className="absolute left-8 md:left-1/2 -translate-x-1/2 flex flex-col items-center justify-center">
                    <div
                      className={cn(
                        'relative z-10 flex items-center justify-center w-8 h-8 rounded-full border-4 border-background transition-transform duration-300 group-hover:scale-110 shadow-sm',
                        isLast
                          ? 'bg-primary border-primary/20 ring-4 ring-primary/10'
                          : 'bg-card border-border',
                      )}
                    >
                      {isLast ? (
                        <div className="h-2.5 w-2.5 bg-background rounded-full animate-pulse" />
                      ) : (
                        <Circle className="w-3 h-3 text-muted-foreground fill-muted-foreground/50" />
                      )}
                    </div>
                  </div>

                  {/* Date/Time Marker (Desktop) */}
                  <div
                    className={cn(
                      'hidden md:flex w-1/2 px-12 items-center text-sm text-muted-foreground/60 font-mono',
                      i % 2 === 0 ? 'justify-start' : 'justify-end',
                    )}
                  >
                    {i18n.date(new Date(t.timestamp), {
                      hour: 'numeric',
                      minute: 'numeric',
                      second: 'numeric',
                    })}
                  </div>

                  {/* Content Card */}
                  <div className={cn('w-full md:w-1/2 pl-20 md:pl-0 md:px-12')}>
                    <Card className="bg-card/40 backdrop-blur-sm border-border/40 hover:border-primary/20 hover:bg-card/60 transition-all duration-300 group-hover:shadow-lg relative overflow-hidden">
                      {/* Mobile Time Stamp */}
                      <div className="md:hidden absolute top-3 right-3 text-[10px] font-mono text-muted-foreground/60 bg-muted/30 px-1.5 py-0.5 rounded">
                        {i18n.date(new Date(t.timestamp), {
                          hour: 'numeric',
                          minute: 'numeric',
                        })}
                      </div>

                      <CardContent className="p-5">
                        <div className="flex flex-col gap-1">
                          <div className="flex items-center gap-2 mb-2">
                            <Badge
                              variant={isFirst ? 'secondary' : 'outline'}
                              className="text-[10px] tracking-wider font-normal"
                            >
                              {isFirst ? (
                                <Trans>INITIAL</Trans>
                              ) : (
                                <Trans>UPDATE</Trans>
                              )}
                            </Badge>
                          </div>

                          <div className="font-medium text-base leading-snug text-foreground">
                            {t.title}
                          </div>

                          {/* Diff View */}
                          {i > 0 && titles[i - 1] && (
                            <div className="mt-4 pt-3 border-t border-border/30">
                              <div className="text-[10px] uppercase tracking-widest text-muted-foreground/50 mb-1.5">
                                <Trans>Previous Title</Trans>
                              </div>
                              <div className="text-sm text-muted-foreground line-through decoration-destructive/30 decoration-1">
                                {titles[i - 1].title}
                              </div>
                            </div>
                          )}
                        </div>
                      </CardContent>
                    </Card>
                  </div>
                </motion.div>
              );
            })}

            {/* End Node */}
            <div className="relative flex justify-center py-4">
              <div className="absolute left-8 md:left-1/2 -translate-x-1/2 w-px h-8 bg-gradient-to-b from-border/60 to-transparent -top-8" />
              <div className="md:ml-auto md:mr-auto ml-8 -translate-x-1/2 md:translate-x-0 bg-muted/20 text-[10px] text-muted-foreground px-3 py-1 rounded-full border border-border/20 flex items-center gap-2">
                <ArrowDown className="h-3 w-3" />
                <Trans>End of History</Trans>
              </div>
            </div>
          </div>
        </div>
      )}
    </motion.div>
  );
}
