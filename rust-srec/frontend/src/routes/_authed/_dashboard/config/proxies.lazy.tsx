import { createLazyFileRoute } from '@tanstack/react-router';
import { motion } from 'motion/react';
import { ProxiesPanel } from '@/components/proxies/proxies-panel';
import { containerVariants } from '@/lib/animation';

export const Route = createLazyFileRoute('/_authed/_dashboard/config/proxies')({
  component: ProxiesPage,
});

function ProxiesPage() {
  return (
    <motion.div
      className="flex flex-col gap-6 pb-20"
      variants={containerVariants}
      initial="hidden"
      animate="visible"
    >
      <ProxiesPanel />
    </motion.div>
  );
}
