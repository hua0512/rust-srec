import { useId, useRef, type KeyboardEvent } from 'react';
import { AlertTriangle } from 'lucide-react';
import { Trans } from '@lingui/react/macro';

import { Badge } from '@/components/ui/badge';
import { Label } from '@/components/ui/label';
import { cn } from '@/lib/utils';

/** Option order drives Arrow-key navigation inside the radio group. */
const IMPORT_MODES = ['merge', 'replace'] as const;

export type ImportMode = (typeof IMPORT_MODES)[number];

/**
 * Import strategy picker.
 *
 * Built from real `<button role="radio">` elements inside a `radiogroup` rather than clickable
 * `<div>`s, so the options are reachable by keyboard and announced as a single choice.
 */
export function ImportModeRadioGroup({
  value,
  onValueChange,
}: {
  value: ImportMode;
  onValueChange: (value: ImportMode) => void;
}) {
  const labelId = useId();
  const optionRefs = useRef<Record<ImportMode, HTMLButtonElement | null>>({
    merge: null,
    replace: null,
  });

  // Only the checked radio stays in the tab order, so Arrow keys have to move
  // both the selection and the focus to the sibling option.
  const handleKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    const forward = event.key === 'ArrowDown' || event.key === 'ArrowRight';
    const backward = event.key === 'ArrowUp' || event.key === 'ArrowLeft';
    if (!forward && !backward) return;

    event.preventDefault();
    const offset = forward ? 1 : IMPORT_MODES.length - 1;
    const next =
      IMPORT_MODES[
        (IMPORT_MODES.indexOf(value) + offset) % IMPORT_MODES.length
      ];
    onValueChange(next);
    optionRefs.current[next]?.focus();
  };

  return (
    <div className="space-y-4">
      <Label id={labelId} className="text-sm font-semibold">
        <Trans>Import Strategy</Trans>
      </Label>

      <div role="radiogroup" aria-labelledby={labelId} className="grid gap-3">
        <button
          type="button"
          role="radio"
          aria-checked={value === 'merge'}
          tabIndex={value === 'merge' ? 0 : -1}
          ref={(node) => {
            optionRefs.current.merge = node;
          }}
          onClick={() => onValueChange('merge')}
          onKeyDown={handleKeyDown}
          className={cn(
            'relative flex cursor-pointer items-start gap-3 rounded-lg border p-4 text-left transition-all hover:bg-muted/50',
            'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2',
            value === 'merge'
              ? 'border-primary bg-primary/5 ring-1 ring-primary'
              : 'opacity-80 hover:opacity-100',
          )}
        >
          <span className="flex h-5 items-center">
            <span
              aria-hidden
              className={cn(
                'h-4 w-4 rounded-full border border-primary flex items-center justify-center',
                value === 'merge' ? 'bg-primary' : 'bg-transparent',
              )}
            >
              {value === 'merge' && (
                <span className="h-1.5 w-1.5 rounded-full bg-primary-foreground" />
              )}
            </span>
          </span>
          <span className="grid gap-1">
            <span className="font-semibold text-sm flex items-center gap-2">
              <Trans>Merge Changes</Trans>
              <Badge
                variant="secondary"
                className="text-[10px] h-5 bg-blue-500/10 text-blue-600 border-blue-200 dark:border-blue-800"
              >
                <Trans>Recommended</Trans>
              </Badge>
            </span>
            <span className="block text-xs text-muted-foreground leading-relaxed">
              <Trans>
                Updates existing items and creates new ones. Nothing is deleted.
              </Trans>
            </span>
          </span>
        </button>

        <button
          type="button"
          role="radio"
          aria-checked={value === 'replace'}
          tabIndex={value === 'replace' ? 0 : -1}
          ref={(node) => {
            optionRefs.current.replace = node;
          }}
          onClick={() => onValueChange('replace')}
          onKeyDown={handleKeyDown}
          className={cn(
            'relative flex cursor-pointer items-start gap-3 rounded-lg border p-4 text-left transition-all hover:bg-red-500/5',
            'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2',
            value === 'replace'
              ? 'border-red-500 bg-red-500/5 ring-1 ring-red-500'
              : 'opacity-80 hover:opacity-100',
          )}
        >
          <span className="flex h-5 items-center">
            <span
              aria-hidden
              className={cn(
                'h-4 w-4 rounded-full border border-primary flex items-center justify-center',
                value === 'replace'
                  ? 'bg-red-500 border-red-500'
                  : 'bg-transparent border-muted-foreground',
              )}
            >
              {value === 'replace' && (
                <span className="h-1.5 w-1.5 rounded-full bg-white" />
              )}
            </span>
          </span>
          <span className="grid gap-1">
            <span className="font-semibold text-sm flex items-center gap-2 text-red-600 dark:text-red-400">
              <Trans>Replace All</Trans>
              <AlertTriangle className="h-3.5 w-3.5" />
            </span>
            <span className="block text-xs text-muted-foreground leading-relaxed">
              <Trans>
                Deletes all existing configurations before importing. This
                action cannot be undone.
              </Trans>
            </span>
          </span>
        </button>
      </div>
    </div>
  );
}
