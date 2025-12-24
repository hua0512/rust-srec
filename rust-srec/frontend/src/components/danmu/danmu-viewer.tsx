import { useEffect, useState, useMemo, useRef } from 'react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import {
  Search,
  MessageSquare,
  Clock,
  Filter,
  AlertCircle,
  Loader2,
  ListFilter,
  User,
} from 'lucide-react';
import { formatDuration } from '@/lib/format';
import { Card } from '@/components/ui/card';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';

interface DanmuViewerProps {
  url: string;
  title: string;
  onClose?: () => void;
}

interface DanmuComment {
  time: number; // Seconds from start
  mode: number;
  size: number;
  color: number;
  timestamp: number; // Unix timestamp
  pool: number;
  userHash: string;
  rowId: string;
  username?: string;
  content: string;
}

type FilterMode = 'all' | 'scrolling' | 'top' | 'bottom';

export function DanmuViewer({ url, title }: DanmuViewerProps) {
  const [comments, setComments] = useState<DanmuComment[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState('');
  const [filterMode, setFilterMode] = useState<FilterMode>('all');

  useEffect(() => {
    const fetchAndParseDanmu = async () => {
      try {
        setLoading(true);
        const response = await fetch(url);
        if (!response.ok) throw new Error('Failed to fetch danmu file');

        const text = await response.text();
        const parser = new DOMParser();
        const xmlDoc = parser.parseFromString(text, 'text/xml');

        const dTags = xmlDoc.getElementsByTagName('d');
        const parsedComments: DanmuComment[] = [];

        for (let i = 0; i < dTags.length; i++) {
          const d = dTags[i];
          const p = d.getAttribute('p');
          if (!p) continue;

          const parts = p.split(',');
          if (parts.length < 8) continue;

          // Parse rowId and username from the last part
          const lastPart = parts[7];
          const spaceIndex = lastPart.indexOf(' ');
          let rowId = lastPart;
          let username = '';

          if (spaceIndex !== -1) {
            rowId = lastPart.substring(0, spaceIndex);
            const userPart = lastPart.substring(spaceIndex + 1);
            if (userPart.startsWith('user=')) {
              username = userPart.substring(5);
            }
          }

          parsedComments.push({
            time: parseFloat(parts[0]),
            mode: parseInt(parts[1]),
            size: parseInt(parts[2]),
            color: parseInt(parts[3]),
            timestamp: parseInt(parts[4]),
            pool: parseInt(parts[5]),
            userHash: parts[6],
            rowId,
            username,
            content: d.textContent || '',
          });
        }

        parsedComments.sort((a, b) => a.time - b.time);
        setComments(parsedComments);
      } catch (err) {
        setError(err instanceof Error ? err.message : 'Unknown error');
      } finally {
        setLoading(false);
      }
    };

    fetchAndParseDanmu();
  }, [url]);

  const filteredComments = useMemo(() => {
    return comments.filter((c) => {
      const contentMatch = c.content
        .toLowerCase()
        .includes(searchQuery.toLowerCase());
      const userMatch = c.username
        ?.toLowerCase()
        .includes(searchQuery.toLowerCase());
      const matchesSearch = contentMatch || userMatch;
      if (!matchesSearch) return false;

      if (filterMode === 'all') return true;
      if (filterMode === 'scrolling')
        return c.mode === 1 || c.mode === 2 || c.mode === 3;
      if (filterMode === 'top') return c.mode === 5;
      if (filterMode === 'bottom') return c.mode === 4;

      return true;
    });
  }, [comments, searchQuery, filterMode]);

  // Virtual Scrolling Constants
  const rowHeight = 44;
  const containerHeight = 600 - 64 - 48 - 32; // Card height minus header, toolbar, and footer
  const [scrollTop, setScrollTop] = useState(0);
  const containerRef = useRef<HTMLDivElement>(null);

  const handleScroll = (e: React.UIEvent<HTMLDivElement>) => {
    setScrollTop(e.currentTarget.scrollTop);
  };

  const virtualData = useMemo(() => {
    const totalItems = filteredComments.length;
    const startIndex = Math.max(0, Math.floor(scrollTop / rowHeight) - 5);
    const endIndex = Math.min(
      totalItems,
      Math.ceil((scrollTop + containerHeight) / rowHeight) + 5,
    );

    return {
      items: filteredComments.slice(startIndex, endIndex),
      start: startIndex,
    };
  }, [filteredComments, scrollTop, containerHeight]);

  return (
    <Card className="flex flex-col h-[85vh] md:h-[600px] w-full overflow-hidden border-border/50 shadow-xl bg-card text-card-foreground">
      {/* Header */}
      <header className="flex items-center p-4 border-b bg-muted/30 gap-4 shrink-0 h-16 w-full">
        <div className="flex h-10 w-10 items-center justify-center rounded-full bg-primary/10 text-primary shrink-0 border border-primary/20">
          <MessageSquare className="h-5 w-5" />
        </div>
        <div className="min-w-0 flex-1">
          <h3
            className="font-bold text-sm tracking-tight truncate leading-tight"
            title={title}
          >
            {title}
          </h3>
          <div className="flex items-center gap-2 mt-0.5">
            <span className="inline-block w-1.5 h-1.5 rounded-full bg-green-500 animate-pulse" />
            <span className="text-[11px] font-medium text-muted-foreground uppercase tracking-wider">
              {comments.length.toLocaleString()} messages
            </span>
          </div>
        </div>
      </header>

      {/* Toolbar - Search & Filter */}
      <div className="flex items-center p-2 px-4 border-b bg-background gap-3 shrink-0 h-12">
        <div className="relative flex-1 group flex items-center">
          <Input
            placeholder="Search comments..."
            className="pl-9 h-8 w-full bg-muted/20 border-border/40 focus-visible:ring-1 focus-visible:ring-primary/20 transition-shadow text-xs rounded-full"
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
          />
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground/60 transition-colors group-focus-within:text-primary pointer-events-none z-10" />
        </div>

        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              variant="ghost"
              size="sm"
              className="h-8 gap-2 text-xs font-medium text-muted-foreground hover:text-foreground"
            >
              <ListFilter className="h-3.5 w-3.5" />
              <span>
                {filterMode === 'all'
                  ? 'Filter'
                  : filterMode.charAt(0).toUpperCase() + filterMode.slice(1)}
              </span>
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-40">
            <DropdownMenuItem
              onClick={() => setFilterMode('all')}
              className="text-xs"
            >
              All Types {filterMode === 'all' && '✓'}
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={() => setFilterMode('scrolling')}
              className="text-xs"
            >
              Scrolling {filterMode === 'scrolling' && '✓'}
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={() => setFilterMode('top')}
              className="text-xs"
            >
              Top Fixed {filterMode === 'top' && '✓'}
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={() => setFilterMode('bottom')}
              className="text-xs"
            >
              Bottom Fixed {filterMode === 'bottom' && '✓'}
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      {/* Main List */}
      <main className="flex-1 overflow-hidden relative bg-background/20">
        {loading ? (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-4">
            <Loader2 className="h-10 w-10 animate-spin text-primary opacity-30" />
            <p className="text-xs font-medium text-muted-foreground tracking-widest uppercase">
              Loading Danmu
            </p>
          </div>
        ) : error ? (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-4 p-8 text-center">
            <div className="h-16 w-16 rounded-full bg-destructive/10 flex items-center justify-center text-destructive">
              <AlertCircle className="h-8 w-8" />
            </div>
            <div className="space-y-1">
              <p className="font-bold text-foreground">
                Failed to load content
              </p>
              <p className="text-xs text-muted-foreground max-w-[300px] leading-relaxed">
                {error}
              </p>
            </div>
          </div>
        ) : (
          <div
            className="h-full overflow-y-auto scrollbar-thin scrollbar-thumb-border hover:scrollbar-thumb-muted-foreground/30 transition-colors"
            ref={containerRef}
            onScroll={handleScroll}
          >
            <div
              style={{
                height: `${rowHeight * filteredComments.length}px`,
                position: 'relative',
              }}
            >
              <div
                className="absolute inset-x-0 top-0"
                style={{
                  transform: `translateY(${virtualData.start * rowHeight}px)`,
                }}
              >
                {filteredComments.length === 0 ? (
                  <div className="py-24 flex flex-col items-center justify-center text-muted-foreground/30 gap-3">
                    <Filter className="h-12 w-12 stroke-[1px]" />
                    <p className="text-sm font-medium tracking-tight">
                      No results found
                    </p>
                  </div>
                ) : (
                  virtualData.items.map((comment, index) => {
                    const actualIndex = virtualData.start + index;
                    return (
                      <div
                        key={`${comment.rowId}-${actualIndex}`}
                        className="group flex gap-4 px-4 py-2 hover:bg-muted/50 transition-colors h-[44px] items-center border-b border-border/10 last:border-0"
                      >
                        <div className="w-16 shrink-0 font-mono text-[10px] text-muted-foreground/50 tabular-nums flex items-center gap-1.5 group-hover:text-muted-foreground transition-colors">
                          <Clock className="h-3 w-3 opacity-50" />
                          {formatDuration(comment.time)}
                        </div>

                        <div className="min-w-0 flex-1 flex items-center gap-3">
                          <div className="flex items-center gap-2 min-w-0">
                            {comment.username ? (
                              <div className="flex items-center gap-1.5 shrink-0">
                                <span className="p-0.5 rounded bg-primary/20 text-primary text-[10px] font-bold tracking-tight px-1.5 uppercase max-w-[100px] truncate border border-primary/20">
                                  {comment.username}
                                </span>
                              </div>
                            ) : (
                              <div className="flex items-center gap-1.5 shrink-0 text-muted-foreground/40">
                                <User className="h-3.5 w-3.5" />
                                <span className="text-[10px] font-mono">
                                  {comment.userHash}
                                </span>
                              </div>
                            )}
                            <div className="text-[13px] font-medium text-foreground/90 group-hover:text-foreground transition-colors truncate">
                              {comment.content}
                            </div>
                          </div>
                        </div>

                        <div className="shrink-0 flex items-center gap-2 opacity-0 group-hover:opacity-100 transition-opacity">
                          <Badge
                            variant="secondary"
                            className="text-[9px] h-5 tabular-nums font-mono px-1.5 bg-muted transition-all"
                          >
                            {new Date(comment.timestamp * 1000).toLocaleString(
                              [],
                              {
                                month: 'short',
                                day: 'numeric',
                                hour: '2-digit',
                                minute: '2-digit',
                              },
                            )}
                          </Badge>
                        </div>
                      </div>
                    );
                  })
                )}
              </div>
            </div>
          </div>
        )}
      </main>

      {/* Footer */}
      <footer className="px-4 py-2 border-t bg-muted/20 flex items-center justify-between h-8 shrink-0">
        <span className="text-[10px] text-muted-foreground/60 font-mono tracking-widest uppercase">
          Anonymized Metadata • UTF-8 Format
        </span>
        <span className="text-[10px] font-bold text-muted-foreground/40 font-mono">
          RUST-SREC PRO VIEWER
        </span>
      </footer>
    </Card>
  );
}
