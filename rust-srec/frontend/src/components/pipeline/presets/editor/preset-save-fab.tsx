import { Button } from '@/components/ui/button';
import { Save } from 'lucide-react';
import { Trans } from '@lingui/react/macro';
import { motion, AnimatePresence } from 'motion/react';

interface PresetSaveFabProps {
  isDirty: boolean;
  isUpdating: boolean;
  onSave: () => void;
}

export function PresetSaveFab({
  isDirty,
  isUpdating,
  onSave,
}: PresetSaveFabProps) {
  return (
    <AnimatePresence>
      {isDirty && (
        <motion.div
          initial={{ opacity: 0, y: 50, scale: 0.9 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          exit={{ opacity: 0, y: 50, scale: 0.9 }}
          className="fixed bottom-10 right-10 z-50"
        >
          <Button
            onClick={onSave}
            disabled={isUpdating}
            size="lg"
            className="rounded-full shadow-2xl h-16 px-8 text-base font-semibold bg-primary hover:bg-primary/90 transition-all hover:scale-105"
          >
            {isUpdating ? (
              <div className="h-5 w-5 border-2 border-primary-foreground border-t-transparent rounded-full animate-spin mr-3" />
            ) : (
              <Save className="mr-3 h-5 w-5" />
            )}
            <Trans>Save Changes</Trans>
          </Button>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
