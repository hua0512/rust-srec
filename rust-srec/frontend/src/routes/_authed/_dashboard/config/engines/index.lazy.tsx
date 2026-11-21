import { createLazyFileRoute } from '@tanstack/react-router';
import { motion } from 'motion/react';
import { EnginesPanel } from '@/components/config/engines/engines-panel';
import { containerVariants, itemVariants } from '@/lib/animation';

export const Route = createLazyFileRoute('/_authed/_dashboard/config/engines/')(
  {
    component: EnginesConfigPage,
  },
);

function EnginesConfigPage() {
  return (
    <motion.div
      className="flex flex-col gap-6 pb-20"
      variants={containerVariants}
      initial="hidden"
      animate="visible"
    >
      <motion.div variants={itemVariants}>
        <EnginesPanel />
      </motion.div>
    </motion.div>
  );
}
