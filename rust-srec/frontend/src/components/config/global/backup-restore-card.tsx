import { useState, useRef } from 'react';
import { SettingsCard } from '../settings-card';
import { Button } from '@/components/ui/button';
import {
  Download,
  Upload,
  Archive,
  Loader2,
  CheckCircle2,
  AlertTriangle,
  FileJson,
  Info,
  ArrowRight,
  RefreshCw,
  FileUp,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { toast } from 'sonner';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Label } from '@/components/ui/label';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { Badge } from '@/components/ui/badge';
import { cn } from '@/lib/utils';
import { exportConfig, importConfig } from '@/server/functions';
import { motion, AnimatePresence } from 'motion/react';

type ImportMode = 'merge' | 'replace';

interface ImportStats {
  templates_created: number;
  templates_updated: number;
  templates_deleted: number;
  streamers_created: number;
  streamers_updated: number;
  streamers_deleted: number;
  engines_created: number;
  engines_updated: number;
  engines_deleted: number;
  platforms_updated: number;
  channels_created: number;
  channels_updated: number;
  channels_deleted: number;
}

interface ImportResult {
  success: boolean;
  message: string;
  stats: ImportStats;
}

export function BackupRestoreCard() {
  const [showImportDialog, setShowImportDialog] = useState(false);
  const [importResult, setImportResult] = useState<ImportResult | null>(null);
  const [selectedFile, setSelectedFile] = useState<File | null>(null);
  const [importMode, setImportMode] = useState<ImportMode>('merge');
  const fileInputRef = useRef<HTMLInputElement>(null);
  const queryClient = useQueryClient();

  const exportMutation = useMutation({
    mutationFn: async () => {
      const configData = await exportConfig();
      return configData;
    },
    onSuccess: (data) => {
      const blob = new Blob([JSON.stringify(data, null, 2)], {
        type: 'application/json',
      });

      const date = new Date().toISOString().slice(0, 10);
      const filename = `rust-srec-backup-${date}.json`;

      const url = window.URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = filename;
      document.body.appendChild(a);
      a.click();
      window.URL.revokeObjectURL(url);
      document.body.removeChild(a);

      toast.success(t`Configuration exported successfully`);
    },
    onError: (error: any) => {
      toast.error(error.message || t`Failed to export configuration`);
    },
  });

  const importMutation = useMutation({
    mutationFn: async (vars: { fileContent: string; mode: ImportMode }) => {
      const configData = JSON.parse(vars.fileContent);
      return await importConfig({
        data: {
          config: configData,
          mode: vars.mode,
        },
      });
    },
    onSuccess: (result: any) => {
      setImportResult(result);
      // Invalidate all relevant queries to refresh UI
      queryClient.invalidateQueries({ queryKey: ['config'] });
      queryClient.invalidateQueries({ queryKey: ['streamers'] });
      queryClient.invalidateQueries({ queryKey: ['templates'] });
      queryClient.invalidateQueries({ queryKey: ['engines'] });
      queryClient.invalidateQueries({ queryKey: ['notifications'] });

      toast.success(t`Configuration imported successfully`);
    },
    onError: (error: any) => {
      toast.error(error.message || t`Failed to import configuration`);
      // Don't close dialog on error so user can try again
    },
  });

  const handleFileSelect = (event: React.ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    if (file) {
      if (!file.name.endsWith('.json')) {
        toast.error(t`Please select a JSON file`);
        return;
      }
      setSelectedFile(file);
      setImportMode('merge'); // Reset to merge mode
      setShowImportDialog(true);
      setImportResult(null); // Reset result
    }
    // Reset file input
    if (fileInputRef.current) {
      fileInputRef.current.value = '';
    }
  };

  const handleImport = async () => {
    if (!selectedFile) return;
    const fileContent = await selectedFile.text();
    importMutation.mutate({ fileContent, mode: importMode });
  };

  const closeDialog = () => {
    setShowImportDialog(false);
    setTimeout(() => {
      setSelectedFile(null);
      setImportResult(null);
      importMutation.reset();
    }, 300); // Wait for dialog animation
  };

  const StatItem = ({
    label,
    count,
    type,
  }: {
    label: string;
    count: number;
    type: 'create' | 'update' | 'delete';
  }) => {
    if (count <= 0) return null;

    const colors = {
      create:
        'text-emerald-600 dark:text-emerald-400 bg-emerald-500/10 border-emerald-500/20',
      update:
        'text-blue-600 dark:text-blue-400 bg-blue-500/10 border-blue-500/20',
      delete: 'text-red-600 dark:text-red-400 bg-red-500/10 border-red-500/20',
    };

    const labels = {
      create: t`Created`,
      update: t`Updated`,
      delete: t`Deleted`,
    };

    return (
      <div
        className={cn(
          'flex items-center justify-between py-1.5 px-3 rounded-md text-sm border',
          colors[type],
        )}
      >
        <span className="font-medium">{label}</span>
        <div className="flex items-center gap-2">
          <span className="text-[10px] uppercase font-bold tracking-wider opacity-80">
            {labels[type]}
          </span>
          <Badge
            variant="outline"
            className={cn(
              'border-0 bg-background/50 font-mono text-xs',
              colors[type],
            )}
          >
            {count}
          </Badge>
        </div>
      </div>
    );
  };

  return (
    <>
      <SettingsCard
        title={<Trans>Backup & Restore</Trans>}
        description={<Trans>Manage your application configuration.</Trans>}
        icon={Archive}
        iconColor="text-emerald-500"
        iconBgColor="bg-emerald-500/10"
      >
        <div className="space-y-6">
          <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
            {/* Export Section */}
            <div className="rounded-xl border bg-card p-4 space-y-4 hover:shadow-sm transition-all duration-300 group">
              <div className="flex items-center gap-3">
                <div className="h-10 w-10 rounded-full bg-emerald-500/10 flex items-center justify-center text-emerald-600 group-hover:scale-110 transition-transform">
                  <Download className="h-5 w-5" />
                </div>
                <div>
                  <h3 className="font-medium text-sm">
                    <Trans>Export Configuration</Trans>
                  </h3>
                  <p className="text-xs text-muted-foreground">
                    <Trans>Save settings to a JSON file</Trans>
                  </p>
                </div>
              </div>

              <Button
                variant="outline"
                onClick={() => exportMutation.mutate()}
                disabled={exportMutation.isPending}
                className="w-full gap-2 hover:bg-emerald-500/5 hover:text-emerald-600 hover:border-emerald-200 dark:hover:border-emerald-800 transition-colors"
              >
                {exportMutation.isPending ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  <FileJson className="h-4 w-4" />
                )}
                <Trans>Download Backup</Trans>
              </Button>
            </div>

            {/* Import Section */}
            <div className="rounded-xl border bg-card p-4 space-y-4 hover:shadow-sm transition-all duration-300 group">
              <div className="flex items-center gap-3">
                <div className="h-10 w-10 rounded-full bg-blue-500/10 flex items-center justify-center text-blue-600 group-hover:scale-110 transition-transform">
                  <Upload className="h-5 w-5" />
                </div>
                <div>
                  <h3 className="font-medium text-sm">
                    <Trans>Import Configuration</Trans>
                  </h3>
                  <p className="text-xs text-muted-foreground">
                    <Trans>Restore from a backup file</Trans>
                  </p>
                </div>
              </div>

              <input
                ref={fileInputRef}
                type="file"
                accept=".json"
                onChange={handleFileSelect}
                className="hidden"
              />
              <Button
                variant="outline"
                onClick={() => fileInputRef.current?.click()}
                disabled={importMutation.isPending}
                className="w-full gap-2 hover:bg-blue-500/5 hover:text-blue-600 hover:border-blue-200 dark:hover:border-blue-800 transition-colors"
              >
                <FileUp className="h-4 w-4" />
                <Trans>Select File</Trans>
              </Button>
            </div>
          </div>

          <div className="rounded-lg bg-muted/40 p-4 text-xs text-muted-foreground flex gap-3 border border-dashed">
            <Info className="h-4 w-4 mt-0.5 shrink-0 text-primary" />
            <p className="leading-relaxed">
              <Trans>
                Exports include all global settings, platforms, templates,
                streamers, engines, and notification channels. Sensitive data
                like passwords might be redacted or encrypted depending on
                platform settings.
              </Trans>
            </p>
          </div>
        </div>
      </SettingsCard>

      <Dialog open={showImportDialog} onOpenChange={closeDialog}>
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>
              {importResult ? (
                <div className="flex items-center gap-2 text-emerald-600 dark:text-emerald-500">
                  <CheckCircle2 className="h-5 w-5" />
                  <Trans>Import Successful</Trans>
                </div>
              ) : (
                <Trans>Import Configuration</Trans>
              )}
            </DialogTitle>
            <DialogDescription>
              {importResult ? (
                <Trans>
                  The configuration has been successfully processed. Here is a
                  summary of the changes:
                </Trans>
              ) : (
                <Trans>
                  Configure how you want to import data from the selected backup
                  file.
                </Trans>
              )}
            </DialogDescription>
          </DialogHeader>

          <AnimatePresence mode="wait">
            {importResult ? (
              <motion.div
                initial={{ opacity: 0, y: 10 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -10 }}
                className="space-y-4 py-2"
              >
                <div className="grid gap-2 max-h-[60vh] overflow-y-auto pr-1">
                  {/* Templates */}
                  <StatItem
                    label={t`Templates`}
                    count={importResult.stats.templates_created}
                    type="create"
                  />
                  <StatItem
                    label={t`Templates`}
                    count={importResult.stats.templates_updated}
                    type="update"
                  />
                  <StatItem
                    label={t`Templates`}
                    count={importResult.stats.templates_deleted}
                    type="delete"
                  />

                  {/* Streamers */}
                  <StatItem
                    label={t`Streamers`}
                    count={importResult.stats.streamers_created}
                    type="create"
                  />
                  <StatItem
                    label={t`Streamers`}
                    count={importResult.stats.streamers_updated}
                    type="update"
                  />
                  <StatItem
                    label={t`Streamers`}
                    count={importResult.stats.streamers_deleted}
                    type="delete"
                  />

                  {/* Engines */}
                  <StatItem
                    label={t`Engines`}
                    count={importResult.stats.engines_created}
                    type="create"
                  />
                  <StatItem
                    label={t`Engines`}
                    count={importResult.stats.engines_updated}
                    type="update"
                  />
                  <StatItem
                    label={t`Engines`}
                    count={importResult.stats.engines_deleted}
                    type="delete"
                  />

                  {/* Platforms */}
                  <StatItem
                    label={t`Platforms`}
                    count={importResult.stats.platforms_updated}
                    type="update"
                  />

                  {/* Channels */}
                  <StatItem
                    label={t`Channels`}
                    count={importResult.stats.channels_created}
                    type="create"
                  />
                  <StatItem
                    label={t`Channels`}
                    count={importResult.stats.channels_updated}
                    type="update"
                  />
                  <StatItem
                    label={t`Channels`}
                    count={importResult.stats.channels_deleted}
                    type="delete"
                  />

                  {Object.values(importResult.stats).every(
                    (val) => val === 0,
                  ) && (
                    <div className="flex flex-col items-center justify-center py-8 text-muted-foreground bg-muted/30 rounded-lg border border-dashed">
                      <RefreshCw className="h-8 w-8 mb-2 opacity-50" />
                      <span className="text-sm font-medium">
                        <Trans>No changes detected</Trans>
                      </span>
                      <span className="text-xs opacity-70">
                        <Trans>The configuration matches existing data</Trans>
                      </span>
                    </div>
                  )}
                </div>
              </motion.div>
            ) : (
              <motion.div
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                className="space-y-6 py-2"
              >
                {selectedFile && (
                  <div className="flex items-center gap-4 rounded-xl border bg-muted/30 p-4">
                    <div className="flex h-12 w-12 items-center justify-center rounded-lg bg-primary/10 text-primary shadow-sm border border-primary/20">
                      <FileJson className="h-6 w-6" />
                    </div>
                    <div className="flex-1 overflow-hidden">
                      <p className="text-sm font-semibold truncate">
                        {selectedFile.name}
                      </p>
                      <div className="flex items-center gap-2 mt-1">
                        <Badge
                          variant="outline"
                          className="text-[10px] h-5 bg-background"
                        >
                          JSON
                        </Badge>
                        <span className="text-xs text-muted-foreground">
                          {(selectedFile.size / 1024).toFixed(1)} KB
                        </span>
                      </div>
                    </div>
                  </div>
                )}

                <div className="space-y-4">
                  <Label className="text-sm font-semibold">
                    <Trans>Import Strategy</Trans>
                  </Label>

                  <div className="grid gap-3">
                    <div
                      onClick={() => setImportMode('merge')}
                      className={cn(
                        'relative flex cursor-pointer items-start gap-3 rounded-lg border p-4 transition-all hover:bg-muted/50',
                        importMode === 'merge'
                          ? 'border-primary bg-primary/5 ring-1 ring-primary'
                          : 'opacity-80 hover:opacity-100',
                      )}
                    >
                      <div className="flex h-5 items-center">
                        <div
                          className={cn(
                            'h-4 w-4 rounded-full border border-primary flex items-center justify-center',
                            importMode === 'merge'
                              ? 'bg-primary'
                              : 'bg-transparent',
                          )}
                        >
                          {importMode === 'merge' && (
                            <div className="h-1.5 w-1.5 rounded-full bg-primary-foreground" />
                          )}
                        </div>
                      </div>
                      <div className="grid gap-1">
                        <div className="font-semibold text-sm flex items-center gap-2">
                          <Trans>Merge Changes</Trans>
                          <Badge
                            variant="secondary"
                            className="text-[10px] h-5 bg-blue-500/10 text-blue-600 border-blue-200 dark:border-blue-800"
                          >
                            <Trans>Recommended</Trans>
                          </Badge>
                        </div>
                        <p className="text-xs text-muted-foreground leading-relaxed">
                          <Trans>
                            Updates existing items and creates new ones. Nothing
                            is deleted.
                          </Trans>
                        </p>
                      </div>
                    </div>

                    <div
                      onClick={() => setImportMode('replace')}
                      className={cn(
                        'relative flex cursor-pointer items-start gap-3 rounded-lg border p-4 transition-all hover:bg-red-500/5',
                        importMode === 'replace'
                          ? 'border-red-500 bg-red-500/5 ring-1 ring-red-500'
                          : 'opacity-80 hover:opacity-100',
                      )}
                    >
                      <div className="flex h-5 items-center">
                        <div
                          className={cn(
                            'h-4 w-4 rounded-full border border-primary flex items-center justify-center',
                            importMode === 'replace'
                              ? 'bg-red-500 border-red-500'
                              : 'bg-transparent border-muted-foreground',
                          )}
                        >
                          {importMode === 'replace' && (
                            <div className="h-1.5 w-1.5 rounded-full bg-white" />
                          )}
                        </div>
                      </div>
                      <div className="grid gap-1">
                        <div className="font-semibold text-sm flex items-center gap-2 text-red-600 dark:text-red-400">
                          <Trans>Replace All</Trans>
                          <AlertTriangle className="h-3.5 w-3.5" />
                        </div>
                        <p className="text-xs text-muted-foreground leading-relaxed">
                          <Trans>
                            Deletes all existing configurations before
                            importing. This action cannot be undone.
                          </Trans>
                        </p>
                      </div>
                    </div>
                  </div>
                </div>
              </motion.div>
            )}
          </AnimatePresence>

          <DialogFooter className="gap-2 sm:gap-0 mt-2">
            {importResult ? (
              <Button
                onClick={closeDialog}
                className="w-full sm:w-auto min-w-[100px]"
              >
                <Trans>Close</Trans>
              </Button>
            ) : (
              <>
                <Button
                  variant="ghost"
                  onClick={closeDialog}
                  disabled={importMutation.isPending}
                >
                  <Trans>Cancel</Trans>
                </Button>
                <Button
                  onClick={handleImport}
                  disabled={importMutation.isPending}
                  variant={importMode === 'replace' ? 'destructive' : 'default'}
                  className="min-w-[140px]"
                >
                  {importMutation.isPending ? (
                    <>
                      <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                      <Trans>Importing...</Trans>
                    </>
                  ) : (
                    <>
                      <Trans>Confirm Import</Trans>
                      <ArrowRight className="ml-2 h-4 w-4" />
                    </>
                  )}
                </Button>
              </>
            )}
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  );
}
