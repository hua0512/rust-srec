import { memo } from 'react';
import { useFormContext } from 'react-hook-form';
import { Trans } from '@lingui/react/macro';
import { Layers } from 'lucide-react';
import { SettingsCard } from '../settings-card';
import { PipelineTabsSection } from '../shared/pipeline-tabs-section';

export const PipelineConfigCard = memo(() => {
  const form = useFormContext();

  return (
    <div className="space-y-6">
      <SettingsCard
        title={<Trans>Pipeline Configuration</Trans>}
        description={
          <Trans>
            Default pipeline flow. Configure the sequence of processors for new
            jobs.
          </Trans>
        }
        icon={Layers}
        iconColor="text-orange-500"
        iconBgColor="bg-orange-500/10"
      >
        <div className="space-y-6">
          <PipelineTabsSection
            form={form}
            names={{
              perSegment: 'pipeline',
              paired: 'paired_segment_pipeline',
              session: 'session_complete_pipeline',
            }}
            dagNames={{
              perSegment: 'global_pipeline',
              paired: 'global_paired_pipeline',
              session: 'global_session_pipeline',
            }}
          />
        </div>
      </SettingsCard>
    </div>
  );
});

PipelineConfigCard.displayName = 'PipelineConfigCard';
