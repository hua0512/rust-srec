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
} from 'lucide-react';
import { formatDuration } from '@/lib/format';
import { Card } from '@/components/ui/card';

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
  content: string;
}

export function DanmuViewer({
  url,
  title,
  onClose: _onClose,
}: DanmuViewerProps) {
  const [comments, setComments] = useState<DanmuComment[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState('');
  const [filterMode, _setFilterMode] = useState<
    'all' | 'scrolling' | 'top' | 'bottom'
  >('all');

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

          parsedComments.push({
            time: parseFloat(parts[0]),
            mode: parseInt(parts[1]),
            size: parseInt(parts[2]),
            color: parseInt(parts[3]),
            timestamp: parseInt(parts[4]),
            pool: parseInt(parts[5]),
            userHash: parts[6],
            rowId: parts[7],
            content: d.textContent || '',
          });
        }

        // Sort by time
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
      const matchesSearch = c.content
        .toLowerCase()
        .includes(searchQuery.toLowerCase());
      if (!matchesSearch) return false;

      if (filterMode === 'all') return true;
      if (filterMode === 'scrolling')
        return c.mode === 1 || c.mode === 2 || c.mode === 3;
      if (filterMode === 'top') return c.mode === 5;
      if (filterMode === 'bottom') return c.mode === 4;

      return true;
    });
  }, [comments, searchQuery, filterMode]);

  // Virtual Scrolling Logic
  const rowHeight = 40; // Approx height including padding/gap
  const containerHeight = 600 - 80; // Total height minus header/footer approximations
  const [scrollTop, setScrollTop] = useState(0);
  const containerRef = useRef<HTMLDivElement>(null);

  // Optimize scroll handler with requestAnimationFrame if needed,
  // but React's state update is usually fast enough for simple lists.
  const handleScroll = (e: React.UIEvent<HTMLDivElement>) => {
    setScrollTop(e.currentTarget.scrollTop);
  };

  const virtualData = useMemo(() => {
    const totalItems = filteredComments.length;
    const startIndex = Math.max(0, Math.floor(scrollTop / rowHeight) - 5); // Buffer
    const endIndex = Math.min(
      totalItems,
      Math.ceil((scrollTop + containerHeight) / rowHeight) + 5, // Buffer
    );

    return {
      items: filteredComments.slice(startIndex, endIndex),
      start: startIndex,
      end: endIndex,
    };
  }, [filteredComments, scrollTop, rowHeight, containerHeight]);

  return (
    <Card className="flex flex-col h-[600px] w-full max-w-4xl mx-auto overflow-hidden bg-background/95 backdrop-blur-xl border-border/50 shadow-2xl animate-in fade-in zoom-in-95 duration-300">
      {/* Header */}
      <div className="flex items-center justify-between p-4 border-b border-border/40 bg-muted/20">
        <div className="flex items-center gap-3">
          <div className="p-2 rounded-lg bg-pink-500/10 text-pink-500">
            <MessageSquare className="h-5 w-5" />
          </div>
          <div>
            <h3 className="font-semibold text-foreground/90">{title}</h3>
            <p className="text-xs text-muted-foreground flex items-center gap-2">
              <span className="inline-block w-1.5 h-1.5 rounded-full bg-green-500" />
              {comments.length.toLocaleString()} comments
            </p>
          </div>
        </div>
        <div className="flex items-center gap-2">
          <div className="relative">
            <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground/50" />
            <Input
              placeholder="Search comments..."
              className="pl-9 h-9 w-[200px] bg-background/50 border-border/40 focus:bg-background transition-all"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
            />
          </div>
          {/* Placeholder for filter dropdown if needed, for now just a static icon indicating functionality */}
          <Button
            variant="ghost"
            size="icon"
            className="h-9 w-9 text-muted-foreground hover:text-foreground"
          >
            <Filter className="h-4 w-4" />
          </Button>
        </div>
      </div>

      {/* Content */}
      <div className="flex-1 overflow-hidden relative">
        {loading ? (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 text-muted-foreground">
            <Loader2 className="h-8 w-8 animate-spin text-primary" />
            <p className="text-sm font-medium">Parsing XML...</p>
          </div>
        ) : error ? (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 text-destructive p-6 text-center">
            <AlertCircle className="h-10 w-10" />
            <p className="font-medium">Failed to load Danmu</p>
            <p className="text-sm opacity-80">{error}</p>
          </div>
        ) : (
          <div
            className="h-full overflow-y-auto relative"
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
                style={{
                  transform: `translateY(${virtualData.start * rowHeight}px)`,
                  position: 'absolute',
                  top: 0,
                  left: 0,
                  right: 0,
                }}
                className="p-2 space-y-1"
              >
                {filteredComments.length === 0 ? (
                  <div className="py-20 text-center text-muted-foreground/40">
                    <p>No comments found</p>
                  </div>
                ) : (
                  virtualData.items.map((comment, index) => {
                    // Use actual index from filteredComments for stability if needed,
                    // or just the virtual index.
                    const actualIndex = virtualData.start + index;
                    return (
                      <div
                        key={`${comment.rowId}-${actualIndex}`} // Use stable key if possible, rowId should be unique
                        className="group flex gap-3 p-2 rounded-lg hover:bg-muted/40 transition-colors text-sm h-[36px] items-center"
                      >
                        <div className="w-16 shrink-0 font-mono text-xs text-muted-foreground/50 flex items-center gap-1">
                          <Clock className="h-3 w-3" />
                          {formatDuration(comment.time)}
                        </div>
                        <div className="flex-1 min-w-0 break-words font-medium text-foreground/90 group-hover:text-primary transition-colors truncate">
                          {comment.content}
                        </div>
                        <div className="shrink-0 opacity-0 group-hover:opacity-100 transition-opacity">
                          <Badge
                            variant="outline"
                            className="text-[10px] h-5 font-mono text-muted-foreground/60"
                          >
                            {new Date(
                              comment.timestamp * 1000,
                            ).toLocaleTimeString()}
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
      </div>

      <div className="p-2 border-t border-border/40 bg-muted/20 text-[10px] text-muted-foreground text-center">
        User Hashes are anonymized by Bilibili
      </div>
    </Card>
  );
}

// Hex color to rgba helper could be added if we want to support colored danmu
