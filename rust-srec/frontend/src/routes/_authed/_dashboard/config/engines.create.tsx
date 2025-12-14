import { createFileRoute, useNavigate } from '@tanstack/react-router';
import { EngineEditor } from '@/components/config/engines/engine-editor';
import { t } from '@lingui/core/macro';
import { toast } from 'sonner';

export const Route = createFileRoute(
  '/_authed/_dashboard/config/engines/create',
)({
  component: CreateEnginePage,
});

function CreateEnginePage() {
  const navigate = useNavigate();

  return (
    <div className="max-w-5xl p-4 sm:p-6 lg:p-8">
      <EngineEditor
        onSuccess={() => {
          toast.success(t`Engine created successfully`);
          navigate({ to: '/config/engines' });
        }}
      />
    </div>
  );
}
