import { createFileRoute, useNavigate } from '@tanstack/react-router';
import { EngineEditor } from '@/components/config/engines/engine-editor';
import { msg } from '@lingui/core/macro';
import { useLingui } from '@lingui/react';
import { toast } from 'sonner';

export const Route = createFileRoute(
  '/_authed/_dashboard/config/engines/create',
)({
  component: CreateEnginePage,
});

function CreateEnginePage() {
  const navigate = useNavigate();
  const { i18n } = useLingui();

  return (
    <div className="max-w-5xl p-4 sm:p-6 lg:p-8">
      <EngineEditor
        onSuccess={() => {
          toast.success(i18n._(msg`Engine created successfully`));
          void navigate({ to: '/config/engines' });
        }}
      />
    </div>
  );
}
