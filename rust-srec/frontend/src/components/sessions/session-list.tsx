import { motion } from 'motion/react';
import { SessionCard } from './session-card';
import { SessionSchema } from '../../api/schemas';
import { z } from 'zod';
import { containerVariants, itemVariants } from '@/lib/animation';
import { COMPACT_CARD_GRID } from '@/lib/grid';
import { cn } from '@/lib/utils';

type Session = z.infer<typeof SessionSchema>;

interface SessionListProps {
  sessions: Session[];
  selectionMode?: boolean;
  selectedIds?: Set<string>;
  onSelectionChange?: (id: string, selected: boolean) => void;
}

const GRID = cn(COMPACT_CARD_GRID, 'gap-4');

export function SessionList({
  sessions,
  selectionMode,
  selectedIds,
  onSelectionChange,
}: SessionListProps) {
  return (
    <motion.div
      key="list"
      className={GRID}
      variants={containerVariants}
      initial="hidden"
      animate="visible"
      exit="exit"
    >
      {sessions.map((session) => (
        <motion.div key={session.id} variants={itemVariants}>
          <SessionCard
            session={session}
            selectionMode={selectionMode}
            isSelected={selectedIds?.has(session.id)}
            onSelectChange={onSelectionChange}
          />
        </motion.div>
      ))}
    </motion.div>
  );
}
