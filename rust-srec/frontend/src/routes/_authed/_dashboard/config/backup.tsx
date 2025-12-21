import { createFileRoute } from '@tanstack/react-router';
import { motion } from 'motion/react';
import { BackupRestoreCard } from '@/components/config/global/backup-restore-card';

export const Route = createFileRoute('/_authed/_dashboard/config/backup')({
  component: BackupPage,
});

function BackupPage() {
  return (
    <div className="space-y-6">
      <motion.div
        className="grid gap-8"
        initial={{ opacity: 0 }}
        animate={{ opacity: 1 }}
        transition={{ duration: 0.3 }}
      >
        <motion.div
          initial={{ opacity: 0, y: 20 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.3, delay: 0 }}
        >
          <BackupRestoreCard />
        </motion.div>
      </motion.div>
    </div>
  );
}
