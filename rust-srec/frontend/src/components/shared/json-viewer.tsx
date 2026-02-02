import { memo } from 'react';
import { cn } from '@/lib/utils';

export const prettyJson = (payload?: string) => {
  if (!payload) return '';
  try {
    const parsed = JSON.parse(payload);
    return JSON.stringify(parsed, null, 2);
  } catch {
    return payload;
  }
};

interface JsonNodeProps {
  value: unknown;
  depth: number;
  keyName?: string;
}

const JsonNode = memo(({ value, depth, keyName }: JsonNodeProps) => {
  const indent = depth * 16;

  if (value === null) {
    return (
      <div style={{ paddingLeft: indent }} className="flex items-center gap-1">
        {keyName && (
          <>
            <span className="text-violet-500 dark:text-violet-400 text-sm font-mono">
              "{keyName}"
            </span>
            <span className="text-muted-foreground text-sm">:</span>
          </>
        )}
        <span className="text-orange-600 dark:text-orange-400 text-sm font-mono italic">
          null
        </span>
      </div>
    );
  }

  if (typeof value === 'boolean') {
    return (
      <div style={{ paddingLeft: indent }} className="flex items-center gap-1">
        {keyName && (
          <>
            <span className="text-violet-500 dark:text-violet-400 text-sm font-mono">
              "{keyName}"
            </span>
            <span className="text-muted-foreground text-sm">:</span>
          </>
        )}
        <span className="text-orange-600 dark:text-orange-400 text-sm font-mono">
          {value ? 'true' : 'false'}
        </span>
      </div>
    );
  }

  if (typeof value === 'number') {
    return (
      <div style={{ paddingLeft: indent }} className="flex items-center gap-1">
        {keyName && (
          <>
            <span className="text-violet-500 dark:text-violet-400 text-sm font-mono">
              "{keyName}"
            </span>
            <span className="text-muted-foreground text-sm">:</span>
          </>
        )}
        <span className="text-cyan-600 dark:text-cyan-400 text-sm font-mono">
          {value}
        </span>
      </div>
    );
  }

  if (typeof value === 'string') {
    const isUrl = value.startsWith('http://') || value.startsWith('https://');
    const isPath = value.startsWith('/') || value.match(/^[A-Z]:\\/i);
    const isTimestamp =
      value.match(/^\d{4}-\d{2}-\d{2}T/) ||
      (value.match(/^\d{13}$/) && Number(value) > 1000000000000);

    return (
      <div style={{ paddingLeft: indent }} className="flex items-start gap-1">
        {keyName && (
          <>
            <span className="text-violet-500 dark:text-violet-400 text-sm font-mono shrink-0">
              "{keyName}"
            </span>
            <span className="text-muted-foreground text-sm shrink-0">:</span>
          </>
        )}
        <span
          className={cn(
            'text-sm font-mono break-all',
            isUrl
              ? 'text-blue-600 dark:text-blue-400'
              : isPath
                ? 'text-emerald-600 dark:text-emerald-400'
                : isTimestamp
                  ? 'text-amber-600 dark:text-amber-400'
                  : 'text-green-600 dark:text-green-400',
          )}
        >
          "{value}"
        </span>
      </div>
    );
  }

  if (Array.isArray(value)) {
    if (value.length === 0) {
      return (
        <div
          style={{ paddingLeft: indent }}
          className="flex items-center gap-1"
        >
          {keyName && (
            <>
              <span className="text-violet-500 dark:text-violet-400 text-sm font-mono">
                "{keyName}"
              </span>
              <span className="text-muted-foreground text-sm">:</span>
            </>
          )}
          <span className="text-muted-foreground text-sm font-mono">[]</span>
        </div>
      );
    }

    return (
      <div>
        <div
          style={{ paddingLeft: indent }}
          className="flex items-center gap-1"
        >
          {keyName && (
            <>
              <span className="text-violet-500 dark:text-violet-400 text-sm font-mono">
                "{keyName}"
              </span>
              <span className="text-muted-foreground text-sm">:</span>
            </>
          )}
          <span className="text-muted-foreground text-sm font-mono">[</span>
          <span className="text-muted-foreground/60 text-xs ml-1">
            {value.length} items
          </span>
        </div>
        {value.map((item, idx) => (
          <JsonNode key={idx} value={item} depth={depth + 1} />
        ))}
        <div style={{ paddingLeft: indent }}>
          <span className="text-muted-foreground text-sm font-mono">]</span>
        </div>
      </div>
    );
  }

  if (typeof value === 'object') {
    const entries = Object.entries(value as Record<string, unknown>);
    if (entries.length === 0) {
      return (
        <div
          style={{ paddingLeft: indent }}
          className="flex items-center gap-1"
        >
          {keyName && (
            <>
              <span className="text-violet-500 dark:text-violet-400 text-sm font-mono">
                "{keyName}"
              </span>
              <span className="text-muted-foreground text-sm">:</span>
            </>
          )}
          <span className="text-muted-foreground text-sm font-mono">
            {'{}'}
          </span>
        </div>
      );
    }

    return (
      <div>
        <div
          style={{ paddingLeft: indent }}
          className="flex items-center gap-1"
        >
          {keyName && (
            <>
              <span className="text-violet-500 dark:text-violet-400 text-sm font-mono">
                "{keyName}"
              </span>
              <span className="text-muted-foreground text-sm">:</span>
            </>
          )}
          <span className="text-muted-foreground text-sm font-mono">{'{'}</span>
        </div>
        {entries.map(([k, v]) => (
          <JsonNode key={k} value={v} depth={depth + 1} keyName={k} />
        ))}
        <div style={{ paddingLeft: indent }}>
          <span className="text-muted-foreground text-sm font-mono">{'}'}</span>
        </div>
      </div>
    );
  }

  return (
    <div style={{ paddingLeft: indent }}>
      <span className="text-muted-foreground text-sm font-mono">
        {JSON.stringify(value)}
      </span>
    </div>
  );
});
JsonNode.displayName = 'JsonNode';

export const JsonViewer = memo(({ json }: { json?: string }) => {
  if (!json) return null;

  try {
    const parsed = JSON.parse(json);
    return <JsonNode value={parsed} depth={0} />;
  } catch {
    return (
      <span className="text-muted-foreground text-sm font-mono">{json}</span>
    );
  }
});
JsonViewer.displayName = 'JsonViewer';
