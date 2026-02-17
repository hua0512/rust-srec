import { motion } from 'motion/react';
import { SessionCard } from './session-card';
import { SessionSchema } from '../../api/schemas';
import { z } from 'zod';
import { containerVariants, itemVariants } from '@/lib/animation';

type Session = z.infer<typeof SessionSchema>;

interface SessionListProps {
  sessions: Session[];
  token?: string;
  selectionMode?: boolean;
  selectedIds?: Set<string>;
  onSelectionChange?: (id: string, selected: boolean) => void;
}

const GRID =
  'grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 2xl:grid-cols-5 gap-4';

export function SessionList({
  sessions,
  token,
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
            token={token}
            selectionMode={selectionMode}
            isSelected={selectedIds?.has(session.id)}
            onSelectChange={onSelectionChange}
          />
        </motion.div>
      ))}
    </motion.div>
  );
}
