import { motion } from 'motion/react';
import { ListChecks, Plus } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { Button } from '@/components/ui/button';
import { FAB_ANCHOR } from '@/components/shared/save-fab';
import { cn } from '@/lib/utils';

/**
 * The round "create" button floating over a list page. Pages that hide it
 * while selecting render it inside an `AnimatePresence` for its exit.
 */
export function CreateFab({
  label,
  onClick,
}: {
  /** Names the action for screen readers and the tooltip. */
  label: string;
  onClick: () => void;
}) {
  return (
    <motion.div
      className={FAB_ANCHOR}
      initial={{ scale: 0.93, opacity: 0 }}
      animate={{ scale: 1, opacity: 1 }}
      exit={{ scale: 0.93, opacity: 0 }}
      whileHover={{ scale: 1.1 }}
      whileTap={{ scale: 0.9 }}
    >
      <Button
        onClick={onClick}
        aria-label={label}
        title={label}
        size="icon"
        className="h-14 w-14 rounded-full bg-primary p-0 text-primary-foreground shadow-2xl hover:bg-primary/90"
      >
        <Plus className="h-6 w-6" />
      </Button>
    </motion.div>
  );
}

/** The header button that turns a list page's selection mode on and off. */
export function SelectModeButton({
  selectionMode,
  onToggle,
  label,
}: {
  selectionMode: boolean;
  onToggle: () => void;
  /** Says what gets selected, for screen readers. */
  label: string;
}) {
  return (
    <Button
      variant="outline"
      size="sm"
      onClick={onToggle}
      aria-pressed={selectionMode}
      aria-label={label}
      className={cn(
        'h-9 gap-2 whitespace-nowrap rounded-full px-3',
        selectionMode &&
          'border-primary/50 bg-primary/10 text-primary hover:bg-primary/15 hover:text-primary',
      )}
    >
      <ListChecks className="h-4 w-4" />
      <Trans>Select</Trans>
    </Button>
  );
}
