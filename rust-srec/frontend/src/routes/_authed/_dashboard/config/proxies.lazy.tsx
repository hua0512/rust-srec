import { createLazyFileRoute } from '@tanstack/react-router';
import { motion } from 'motion/react';
import { ProxiesPanel } from '@/components/proxies/proxies-panel';

export const Route = createLazyFileRoute('/_authed/_dashboard/config/proxies')({
  component: ProxiesPage,
});

function ProxiesPage() {
  return (
    <motion.div
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.25 }}
      className="max-w-4xl"
    >
      <ProxiesPanel />
    </motion.div>
  );
}
