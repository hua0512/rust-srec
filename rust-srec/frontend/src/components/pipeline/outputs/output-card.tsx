import { useState, useEffect } from 'react';
import {
  Card,
  CardContent,
  CardFooter,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { Button } from '@/components/ui/button';
import {
  MoreHorizontal,
  HardDrive,
  Check,
  Copy,
  CheckCircle2,
  FolderOpen,
  CloudUpload,
  CloudAlert,
  Trash2,
} from 'lucide-react';
import { DropdownMenuSeparator } from '@/components/ui/dropdown-menu';
import { DeleteOutputsDialog } from '@/components/pipeline/outputs/delete-outputs-dialog';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { toast } from 'sonner';
import { msg } from '@lingui/core/macro';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { cn } from '@/lib/utils';
import { getMediaFileTypeMeta } from '@/lib/media-file-type';
import type { MediaOutput } from '@/api/schemas';

interface OutputCardProps {
  output: MediaOutput;
  onDelete?: (outputId: string, deleteFile: boolean) => void;
  selectionMode?: boolean;
  isSelected?: boolean;
  onSelectChange?: (outputId: string, selected: boolean) => void;
}

import { basename, dirname, formatBytes } from '@/lib/format';
import { formatDate } from '@/lib/datetime';

export function OutputCard({
  output,
  onDelete,
  selectionMode = false,
  isSelected = false,
  onSelectChange,
}: OutputCardProps) {
  const { i18n } = useLingui();
  const [copied, setCopied] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);

  // Client-side time for hydration-safe relative time
  const [mounted, setMounted] = useState(false);
  useEffect(() => {
    setMounted(true);
  }, []);

  const handleCopyPath = () => {
    void navigator.clipboard.writeText(output.file_path);
    setCopied(true);
    toast.success(i18n._(msg`File path copied to clipboard`));
    setTimeout(() => setCopied(false), 2000);
  };

  const filename = basename(output.file_path) || i18n._(msg`Unknown File`);
  const directory = dirname(output.file_path) || output.file_path;
  // `format` carries `media_outputs.file_type` (VIDEO, THUMBNAIL, ...), so the
  // icon and colours come from the type map rather than a file extension.
  const typeMeta = getMediaFileTypeMeta(output.format);
  const TypeIcon = typeMeta.icon;
  const hasFailedUpload = output.uploads.some((u) => u.status === 'FAILED');
  const sessionLabel = output.session_id.substring(0, 8);

  const toggleSelection = () => {
    if (selectionMode) {
      onSelectChange?.(output.id, !isSelected);
    }
  };

  return (
    <Card
      onClick={toggleSelection}
      role={selectionMode ? 'checkbox' : undefined}
      aria-checked={selectionMode ? isSelected : undefined}
      tabIndex={selectionMode ? 0 : undefined}
      onKeyDown={(event) => {
        if (selectionMode && (event.key === 'Enter' || event.key === ' ')) {
          event.preventDefault();
          toggleSelection();
        }
      }}
      className={cn(
        // Tighter than the shared Card default (gap-6 py-6): three short rows
        // of metadata otherwise leave most of the card empty.
        'relative h-full flex flex-col gap-3 py-4 transition-all duration-500 hover:-translate-y-1 hover:shadow-2xl hover:shadow-primary/10 group overflow-hidden bg-gradient-to-br from-background/80 to-background/40 backdrop-blur-xl border-border/40 hover:border-primary/20',
        selectionMode && 'cursor-pointer select-none',
        isSelected && 'border-primary/50 ring-2 ring-primary',
      )}
    >
      {selectionMode && (
        <div
          className={cn(
            'absolute right-3 top-3 z-20 flex h-6 w-6 items-center justify-center rounded-full border-2 shadow-sm transition-colors',
            isSelected
              ? 'border-primary bg-primary text-primary-foreground'
              : 'border-border bg-background/90 text-transparent',
          )}
        >
          <Check className="h-3.5 w-3.5" />
        </div>
      )}
      <div className="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent via-primary/40 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700" />

      {/* Hover Glow Effect */}
      <div className="absolute -inset-0.5 bg-gradient-to-br from-primary/5 to-transparent opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-500 pointer-events-none" />

      <CardHeader className="relative flex flex-row items-center gap-3 space-y-0 z-10">
        <div
          className={`p-2 rounded-xl bg-gradient-to-br ${typeMeta.tile} ring-1 ring-inset ring-black/5 dark:ring-white/5 transition-transform duration-500 group-hover:scale-110 group-hover:rotate-3 shrink-0`}
        >
          <TypeIcon className="h-4 w-4" />
        </div>
        {/* min-w-0 lets the timestamp truncate instead of forcing the row wider;
            everything after this column is shrink-0 so it never gets squeezed.
            Deliberately not `uppercase`: that renders the translated type label
            as VIDEO / THUMBNAIL, which is exactly the raw `file_type` value. */}
        <div className="flex-1 min-w-0 flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground/70">
          <span className="shrink-0">{i18n._(typeMeta.label)}</span>
          <span aria-hidden className="shrink-0 opacity-40">
            ·
          </span>
          <span className="truncate">
            {mounted
              ? formatDate(i18n.locale, output.created_at, {
                  dateStyle: 'medium',
                  timeStyle: 'short',
                })
              : formatDate(i18n.locale, output.created_at, {
                  dateStyle: 'medium',
                })}
          </span>
        </div>
        {output.uploads.length > 0 && (
          <Tooltip>
            <TooltipTrigger asChild>
              <div
                className={cn(
                  'p-1.5 rounded-lg border shrink-0',
                  hasFailedUpload
                    ? 'bg-destructive/10 text-destructive border-destructive/20'
                    : 'bg-emerald-500/10 text-emerald-600 border-emerald-500/20 dark:text-emerald-400',
                )}
              >
                {hasFailedUpload ? (
                  <CloudAlert className="h-3.5 w-3.5" />
                ) : (
                  <CloudUpload className="h-3.5 w-3.5" />
                )}
              </div>
            </TooltipTrigger>
            <TooltipContent className="max-w-sm space-y-1.5">
              {output.uploads.map((upload) => (
                <div key={upload.uploader} className="text-xs space-y-0.5">
                  <div className="font-medium">
                    {upload.status === 'COMPLETED' && (
                      <Trans>Uploaded via {upload.uploader}</Trans>
                    )}
                    {upload.status === 'FAILED' && (
                      <Trans>Upload failed via {upload.uploader}</Trans>
                    )}
                    {upload.status === 'SKIPPED' && (
                      <Trans>Upload skipped via {upload.uploader}</Trans>
                    )}
                  </div>
                  {upload.remote_path && (
                    <div className="font-mono break-all opacity-80">
                      {upload.remote_path}
                    </div>
                  )}
                  {upload.completed_at && (
                    <div className="opacity-60">
                      {formatDate(i18n.locale, upload.completed_at, {
                        dateStyle: 'medium',
                        timeStyle: 'short',
                      })}
                    </div>
                  )}
                </div>
              ))}
            </TooltipContent>
          </Tooltip>
        )}
        <DropdownMenu>
          <DropdownMenuTrigger
            asChild
            onClick={(e) => e.stopPropagation()}
            // Hidden while selecting so the row has a single click target.
            className={cn('shrink-0', selectionMode && 'hidden')}
          >
            <Button
              variant="ghost"
              size="icon"
              className="h-8 w-8 -mr-2 text-muted-foreground/40 hover:text-foreground transition-colors"
            >
              <MoreHorizontal className="h-4 w-4" />
              <span className="sr-only">
                <Trans>Open menu</Trans>
              </span>
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-48">
            <DropdownMenuItem onClick={handleCopyPath}>
              {copied ? (
                <CheckCircle2 className="mr-2 h-4 w-4" />
              ) : (
                <Copy className="mr-2 h-4 w-4" />
              )}
              {copied ? <Trans>Copied!</Trans> : <Trans>Copy Path</Trans>}
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={() => {
                void navigator.clipboard.writeText(directory);
                toast.success(i18n._(msg`Directory path copied`));
              }}
            >
              <FolderOpen className="mr-2 h-4 w-4" />{' '}
              <Trans>Copy Directory</Trans>
            </DropdownMenuItem>
            {onDelete && (
              <>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  className="text-destructive focus:text-destructive"
                  onSelect={(e) => e.preventDefault()}
                  onClick={(e) => {
                    e.stopPropagation();
                    setDeleteOpen(true);
                  }}
                >
                  <Trash2 className="mr-2 h-4 w-4" /> <Trans>Delete</Trans>
                </DropdownMenuItem>
              </>
            )}
          </DropdownMenuContent>
        </DropdownMenu>

        {onDelete && (
          <DeleteOutputsDialog
            open={deleteOpen}
            onOpenChange={setDeleteOpen}
            count={1}
            onConfirm={(deleteFile) => {
              setDeleteOpen(false);
              onDelete(output.id, deleteFile);
            }}
          />
        )}
      </CardHeader>

      <CardContent className="relative flex-1 z-10 space-y-1.5">
        {/* The name spans the full card: sharing the header row with the icon
            and menu leaves too little width to reach the descriptive tail of a
            recording filename. */}
        <CardTitle
          className="text-base font-medium truncate tracking-tight text-foreground/90 group-hover:text-primary transition-colors duration-300"
          title={filename}
        >
          {filename}
        </CardTitle>
        {/* The directory only: the file name is the title above, and a
            single truncated line cannot clip a wrapped line in half. */}
        <p
          className="text-[11px] text-muted-foreground/80 truncate font-mono bg-muted/30 px-2 py-1.5 rounded-md"
          title={output.file_path}
        >
          {directory}
        </p>
      </CardContent>

      <CardFooter className="relative text-[11px] flex justify-between items-center gap-3 z-10 border-t border-border/20 mt-auto px-6 pt-3 pb-0 bg-muted/5">
        <span className="flex items-center gap-1.5 font-medium shrink-0">
          <HardDrive className="h-3.5 w-3.5 text-muted-foreground" />
          {formatBytes(output.file_size_bytes)}
        </span>
        <span className="font-mono text-muted-foreground/60 truncate">
          <Trans>Session {sessionLabel}</Trans>
        </span>
      </CardFooter>
    </Card>
  );
}
