import { createFileRoute } from '@tanstack/react-router';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { motion, AnimatePresence } from 'motion/react';
import { t } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { getLoggingConfig, updateLoggingFilter } from '@/server/functions';
import {
  parseFilterDirective,
  serializeFilterDirective,
  LOG_LEVELS,
  type LogLevel,
  type ModuleFilter,
} from '@/api/schemas/logging';
import { Button } from '@/components/ui/button';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Badge } from '@/components/ui/badge';
import { Skeleton } from '@/components/ui/skeleton';
import { toast } from 'sonner';
import { useState, useEffect, useMemo } from 'react';
import {
  Save,
  Bug,
  Info,
  AlertTriangle,
  Terminal,
  XCircle,
  Eye,
  Plus,
  Trash2,
  RotateCcw,
  Sparkles,
} from 'lucide-react';
import { cn } from '@/lib/utils';
import { LogViewer } from '@/components/logging/log-viewer';

export const Route = createFileRoute('/_authed/_dashboard/config/logging')({
  component: LoggingConfigPage,
});

/** i18n module descriptions */
function getModuleDescription(name: string): string {
  const descriptions: Record<string, string> = {
    rust_srec: t`Main application`,
    mesio_engine: t`Download engine (mesio)`,
    flv: t`FLV parser`,
    flv_fix: t`FLV stream fixing pipeline`,
    hls: t`HLS parser`,
    hls_fix: t`HLS stream fixing pipeline`,
    platforms_parser: t`Platform URL extractors`,
    pipeline_common: t`Shared pipeline utilities`,
    sqlx: t`Database queries`,
    reqwest: t`HTTP requests`,
    tower_http: t`HTTP middleware`,
  };
  return descriptions[name] || name;
}

/** Get log level icon */
function getLevelIcon(level: LogLevel) {
  const iconClass = 'w-3.5 h-3.5';
  switch (level) {
    case 'trace':
      return <Terminal className={cn(iconClass, 'text-slate-400')} />;
    case 'debug':
      return <Bug className={cn(iconClass, 'text-blue-400')} />;
    case 'info':
      return <Info className={cn(iconClass, 'text-emerald-400')} />;
    case 'warn':
      return <AlertTriangle className={cn(iconClass, 'text-amber-400')} />;
    case 'error':
      return <XCircle className={cn(iconClass, 'text-rose-400')} />;
    case 'off':
      return <Eye className={cn(iconClass, 'text-muted-foreground/50')} />;
  }
}

/** Get log level color classes */
function getLevelColor(level: LogLevel): string {
  switch (level) {
    case 'trace':
      return 'bg-slate-500/10 text-slate-400 border-slate-500/20';
    case 'debug':
      return 'bg-blue-500/10 text-blue-400 border-blue-500/20';
    case 'info':
      return 'bg-emerald-500/10 text-emerald-400 border-emerald-500/20';
    case 'warn':
      return 'bg-amber-500/10 text-amber-400 border-amber-500/20';
    case 'error':
      return 'bg-rose-500/10 text-rose-400 border-rose-500/20';
    case 'off':
      return 'bg-muted/50 text-muted-foreground border-muted';
  }
}

/** Predefined modules for quick add */
const PREDEFINED_MODULES = [
  'rust_srec',
  'mesio_engine',
  'flv',
  'flv_fix',
  'hls',
  'hls_fix',
  'platforms_parser',
  'pipeline_common',
  'sqlx',
  'reqwest',
  'tower_http',
];

function LoggingConfigPage() {
  const queryClient = useQueryClient();
  const [filters, setFilters] = useState<ModuleFilter[]>([]);
  const [isDirty, setIsDirty] = useState(false);

  const { data: config, isLoading } = useQuery({
    queryKey: ['logging', 'config'],
    queryFn: () => getLoggingConfig(),
  });

  // Initialize filters from config
  useEffect(() => {
    if (config?.filter) {
      setFilters(parseFilterDirective(config.filter));
      setIsDirty(false);
    }
  }, [config?.filter]);

  // Available modules not yet added
  const availableModules = useMemo(() => {
    const usedModules = new Set(filters.map((f) => f.module));
    return PREDEFINED_MODULES.filter((m) => !usedModules.has(m));
  }, [filters]);

  const updateMutation = useMutation({
    mutationFn: (filter: string) => updateLoggingFilter({ data: { filter } }),
    onSuccess: () => {
      toast.success(t`Logging configuration updated`);
      queryClient.invalidateQueries({ queryKey: ['logging', 'config'] });
      setIsDirty(false);
    },
    onError: (error: any) => {
      toast.error(error.message || t`Failed to update logging configuration`);
    },
  });

  const handleLevelChange = (module: string, level: LogLevel) => {
    setFilters((prev) =>
      prev.map((f) => (f.module === module ? { ...f, level } : f)),
    );
    setIsDirty(true);
  };

  const handleAddModule = (module: string) => {
    setFilters((prev) => [...prev, { module, level: 'info' }]);
    setIsDirty(true);
  };

  const handleRemoveModule = (module: string) => {
    setFilters((prev) => prev.filter((f) => f.module !== module));
    setIsDirty(true);
  };

  const handleReset = () => {
    if (config?.filter) {
      setFilters(parseFilterDirective(config.filter));
      setIsDirty(false);
    }
  };

  const handleSave = () => {
    const directive = serializeFilterDirective(filters);
    updateMutation.mutate(directive);
  };

  if (isLoading) {
    return (
      <div className="space-y-6">
        <Skeleton className="h-8 w-64" />
        <Skeleton className="h-[400px] rounded-xl" />
      </div>
    );
  }

  return (
    <div className="space-y-8 pb-32">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="space-y-1">
          <h2 className="text-2xl font-semibold tracking-tight flex items-center gap-3">
            <div className="p-2 rounded-lg bg-gradient-to-br from-violet-500/20 to-purple-500/10 ring-1 ring-violet-500/20">
              <Terminal className="h-5 w-5 text-violet-400" />
            </div>
            <Trans>Logging Configuration</Trans>
          </h2>
          <p className="text-muted-foreground">
            <Trans>Configure log levels for different modules</Trans>
          </p>
        </div>
      </div>

      <motion.div
        initial={{ opacity: 0, y: 20 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.3 }}
      >
        <Card className="border-border/40 bg-gradient-to-b from-card to-card/80 shadow-lg">
          <CardHeader className="pb-4">
            <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4">
              <div>
                <CardTitle className="flex items-center gap-2">
                  <Sparkles className="h-5 w-5 text-primary" />
                  <Trans>Module Log Levels</Trans>
                </CardTitle>
                <CardDescription className="mt-1.5">
                  <Trans>
                    Control verbosity for each module. Lower levels include
                    higher ones.
                  </Trans>
                </CardDescription>
              </div>

              {/* Quick Add Dropdown */}
              {availableModules.length > 0 && (
                <Select onValueChange={handleAddModule}>
                  <SelectTrigger className="w-full sm:w-[200px]">
                    <Plus className="w-4 h-4 mr-2" />
                    <SelectValue placeholder={t`Add module...`} />
                  </SelectTrigger>
                  <SelectContent>
                    {availableModules.map((module) => (
                      <SelectItem key={module} value={module}>
                        <span className="font-mono text-xs">{module}</span>
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              )}
            </div>
          </CardHeader>

          <CardContent className="space-y-3">
            <AnimatePresence mode="popLayout">
              {filters.map((filter, index) => (
                <motion.div
                  key={filter.module}
                  layout
                  initial={{ opacity: 0, scale: 0.9 }}
                  animate={{ opacity: 1, scale: 1 }}
                  exit={{ opacity: 0, scale: 0.9 }}
                  transition={{ duration: 0.2, delay: index * 0.02 }}
                  className="group flex flex-col sm:flex-row sm:items-center gap-3 sm:gap-4 p-3 sm:p-4 rounded-xl border border-border/40 bg-muted/30 hover:bg-muted/50 transition-colors"
                >
                  {/* Module Info */}
                  <div className="flex-1 min-w-0 w-full">
                    <div className="flex flex-wrap items-center gap-2">
                      <code className="text-sm font-semibold text-foreground">
                        {filter.module}
                      </code>
                      <Badge
                        variant="outline"
                        className={cn(
                          'text-[10px] uppercase font-medium',
                          getLevelColor(filter.level),
                        )}
                      >
                        {getLevelIcon(filter.level)}
                        <span className="ml-1">{filter.level}</span>
                      </Badge>
                    </div>
                    <p className="text-xs text-muted-foreground mt-1 truncate">
                      {getModuleDescription(filter.module)}
                    </p>
                  </div>

                  {/* Controls Container */}
                  <div className="flex items-center justify-between sm:justify-end gap-3 w-full sm:w-auto mt-2 sm:mt-0">
                    {/* Level Selector */}
                    <Select
                      value={filter.level}
                      onValueChange={(level: LogLevel) =>
                        handleLevelChange(filter.module, level)
                      }
                    >
                      <SelectTrigger className="w-full sm:w-[130px] h-9">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {LOG_LEVELS.map((level) => (
                          <SelectItem key={level} value={level}>
                            <div className="flex items-center gap-2">
                              {getLevelIcon(level)}
                              <span className="capitalize">{level}</span>
                            </div>
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>

                    {/* Remove Button */}
                    <Button
                      variant="ghost"
                      size="icon"
                      className="text-muted-foreground hover:text-destructive sm:opacity-0 sm:group-hover:opacity-100 transition-opacity h-9 w-9"
                      onClick={() => handleRemoveModule(filter.module)}
                    >
                      <Trash2 className="w-4 h-4" />
                    </Button>
                  </div>
                </motion.div>
              ))}
            </AnimatePresence>

            {filters.length === 0 && (
              <div className="flex flex-col items-center justify-center py-12 text-center">
                <Terminal className="h-12 w-12 text-muted-foreground/30 mb-4" />
                <p className="text-muted-foreground">
                  <Trans>No modules configured</Trans>
                </p>
                <p className="text-sm text-muted-foreground/60 mt-1">
                  <Trans>Add a module above to configure its log level</Trans>
                </p>
              </div>
            )}

            {/* Current Filter Preview */}
            {filters.length > 0 && (
              <div className="mt-6 p-4 rounded-lg bg-muted/20 border border-border/30">
                <p className="text-xs font-medium text-muted-foreground mb-2">
                  <Trans>Current Filter Directive</Trans>
                </p>
                <code className="text-xs text-primary break-all">
                  {serializeFilterDirective(filters) || t`(empty)`}
                </code>
              </div>
            )}
          </CardContent>
        </Card>
      </motion.div>

      {/* Real-Time Log Viewer */}
      <motion.div
        initial={{ opacity: 0, y: 20 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.3, delay: 0.1 }}
      >
        <LogViewer />
      </motion.div>

      {/* Floating Action Buttons */}
      {isDirty && (
        <div className="fixed bottom-8 right-8 z-50 flex items-center gap-3 animate-in fade-in slide-in-from-bottom-4 duration-300">
          <Button variant="outline" onClick={handleReset} className="shadow-lg">
            <RotateCcw className="w-4 h-4 mr-2" />
            <Trans>Reset</Trans>
          </Button>
          <Button
            onClick={handleSave}
            disabled={updateMutation.isPending}
            size="lg"
            className="shadow-2xl shadow-primary/40 hover:shadow-primary/50 transition-all hover:scale-105 active:scale-95 rounded-full px-8 h-14 bg-gradient-to-r from-primary to-primary/90 text-base font-semibold"
          >
            <Save className="w-5 h-5 mr-2" />
            {updateMutation.isPending ? t`Saving...` : t`Save Changes`}
          </Button>
        </div>
      )}
    </div>
  );
}
