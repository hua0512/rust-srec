import { useState } from 'react';
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
import { Badge } from '@/components/ui/badge';
import {
  MoreHorizontal,
  FileVideo,
  HardDrive,
  Calendar,
  Copy,
  CheckCircle2,
  FolderOpen,
} from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { toast } from 'sonner';
import { format, formatDistanceToNow } from 'date-fns';

interface MediaOutput {
  id: string;
  session_id: string;
  file_path: string;
  file_size_bytes: number;
  format: string;
  created_at: string;
}

interface OutputCardProps {
  output: MediaOutput;
}

// Format colors for different file types
const FORMAT_COLORS: Record<string, string> = {
  mp4: 'from-blue-500/10 to-blue-500/5 text-blue-500 border-blue-500/20',
  mkv: 'from-purple-500/10 to-purple-500/5 text-purple-500 border-purple-500/20',
  flv: 'from-orange-500/10 to-orange-500/5 text-orange-500 border-orange-500/20',
  ts: 'from-green-500/10 to-green-500/5 text-green-500 border-green-500/20',
  m4a: 'from-pink-500/10 to-pink-500/5 text-pink-500 border-pink-500/20',
  mp3: 'from-pink-500/10 to-pink-500/5 text-pink-500 border-pink-500/20',
  jpg: 'from-amber-500/10 to-amber-500/5 text-amber-500 border-amber-500/20',
  png: 'from-amber-500/10 to-amber-500/5 text-amber-500 border-amber-500/20',
  webp: 'from-amber-500/10 to-amber-500/5 text-amber-500 border-amber-500/20',
};

// Helper to format bytes
function formatBytes(bytes: number, decimals = 2): string {
  if (!+bytes) return '0 Bytes';
  const k = 1024;
  const dm = decimals < 0 ? 0 : decimals;
  const sizes = ['Bytes', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return `${parseFloat((bytes / Math.pow(k, i)).toFixed(dm))} ${sizes[i]}`;
}

export function OutputCard({ output }: OutputCardProps) {
  const [copied, setCopied] = useState(false);

  const handleCopyPath = () => {
    navigator.clipboard.writeText(output.file_path);
    setCopied(true);
    toast.success('File path copied to clipboard');
    setTimeout(() => setCopied(false), 2000);
  };

  const filename = output.file_path.split(/[\\/]/).pop() || 'Unknown File';
  const formatLower = output.format.toLowerCase();
  const colorClass =
    FORMAT_COLORS[formatLower] ||
    'from-gray-500/10 to-gray-500/5 text-gray-500 border-gray-500/20';

  return (
    <Card className="relative h-full flex flex-col transition-all duration-500 hover:-translate-y-1 hover:shadow-2xl hover:shadow-primary/10 group overflow-hidden bg-gradient-to-br from-background/80 to-background/40 backdrop-blur-xl border-border/40 hover:border-primary/20">
      <div className="absolute inset-x-0 top-0 h-0.5 bg-gradient-to-r from-transparent via-primary/40 to-transparent opacity-0 group-hover:opacity-100 transition-opacity duration-700" />

      {/* Hover Glow Effect */}
      <div className="absolute -inset-0.5 bg-gradient-to-br from-primary/5 to-transparent opacity-0 group-hover:opacity-100 blur-2xl transition-opacity duration-500 pointer-events-none" />

      <CardHeader className="relative flex flex-row items-center gap-4 pb-2 space-y-0 z-10">
        <div
          className={`p-3 rounded-2xl bg-gradient-to-br ${colorClass} ring-1 ring-inset ring-black/5 dark:ring-white/5 transition-transform duration-500 group-hover:scale-110 group-hover:rotate-3`}
        >
          <FileVideo className="h-5 w-5" />
        </div>
        <div className="flex-1 min-w-0 space-y-1">
          <CardTitle
            className="text-base font-medium truncate tracking-tight text-foreground/90 group-hover:text-primary transition-colors duration-300"
            title={filename}
          >
            {filename}
          </CardTitle>
          <div className="flex items-center gap-2">
            <span className="text-[10px] uppercase tracking-wider font-semibold text-muted-foreground/60">
              {formatDistanceToNow(new Date(output.created_at), {
                addSuffix: true,
              })}
            </span>
          </div>
        </div>
        <Badge
          className={`bg-gradient-to-br ${colorClass} border font-mono text-xs uppercase`}
        >
          {output.format}
        </Badge>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
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
                const dir = output.file_path.substring(
                  0,
                  output.file_path.lastIndexOf(
                    /[\\/]/.test(output.file_path)
                      ? output.file_path.includes('\\')
                        ? '\\'
                        : '/'
                      : '/',
                  ),
                );
                navigator.clipboard.writeText(dir);
                toast.success('Directory path copied');
              }}
            >
              <FolderOpen className="mr-2 h-4 w-4" />{' '}
              <Trans>Copy Directory</Trans>
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </CardHeader>

      <CardContent className="relative pb-4 flex-1 z-10">
        <p
          className="text-xs text-muted-foreground/80 line-clamp-2 mb-4 leading-relaxed font-mono bg-muted/30 p-2 rounded-md"
          title={output.file_path}
        >
          {output.file_path}
        </p>

        {/* Stats */}
        <div className="grid grid-cols-2 gap-3">
          <div className="flex items-center gap-2 p-2 rounded-md bg-muted/30 border border-transparent group-hover:border-primary/5 transition-colors">
            <HardDrive className="h-3.5 w-3.5 text-muted-foreground" />
            <div className="flex flex-col">
              <span className="text-[9px] uppercase tracking-wider text-muted-foreground/50">
                Size
              </span>
              <span className="text-xs font-medium">
                {formatBytes(output.file_size_bytes)}
              </span>
            </div>
          </div>
          <div className="flex items-center gap-2 p-2 rounded-md bg-muted/30 border border-transparent group-hover:border-primary/5 transition-colors">
            <Calendar className="h-3.5 w-3.5 text-muted-foreground" />
            <div className="flex flex-col">
              <span className="text-[9px] uppercase tracking-wider text-muted-foreground/50">
                Created
              </span>
              <span className="text-xs font-medium">
                {format(new Date(output.created_at), 'PP')}
              </span>
            </div>
          </div>
        </div>
      </CardContent>

      <CardFooter className="relative pt-0 text-[10px] text-muted-foreground flex justify-between items-center z-10 border-t border-border/20 mt-auto px-6 py-3 bg-muted/5">
        <span className="font-mono opacity-50">
          Session: {output.session_id.substring(0, 8)}
        </span>
        <span className="font-mono opacity-50">
          {formatBytes(output.file_size_bytes)}
        </span>
      </CardFooter>
    </Card>
  );
}
