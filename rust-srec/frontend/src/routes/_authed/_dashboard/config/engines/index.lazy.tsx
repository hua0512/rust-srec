import { createLazyFileRoute } from '@tanstack/react-router';
import { motion } from 'motion/react';
import { EnginesPanel } from '@/components/config/engines/engines-panel';

export const Route = createLazyFileRoute('/_authed/_dashboard/config/engines/')(
  {
    component: EnginesConfigPage,
  },
);

function EnginesConfigPage() {
  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.25 }}
      className="max-w-4xl"
    >
      <EnginesPanel />
    </motion.div>
  );
}
