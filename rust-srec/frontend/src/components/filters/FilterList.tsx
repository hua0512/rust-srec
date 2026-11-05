import { FilterSchema } from '../../api/schemas';
import { z } from 'zod';
import { FilterCard } from './FilterCard';
import { Trans } from '@lingui/react/macro';
import { FilterX } from 'lucide-react';
import { CARD_GRID } from '@/lib/grid';
import { cn } from '@/lib/utils';
import { ListPanelEmpty } from '@/components/shared/list-panel';

type Filter = z.infer<typeof FilterSchema>;

interface FilterListProps {
  filters: Filter[];
  onEdit: (filter: Filter) => void;
  onDelete: (filterId: string) => void;
}

export function FilterList({ filters, onEdit, onDelete }: FilterListProps) {
  if (filters.length === 0) {
    return (
      <ListPanelEmpty
        icon={FilterX}
        title={<Trans>No filters yet</Trans>}
        help={
          <Trans>
            Create a filter to control when this streamer is recorded. You can
            filter by time, keywords, categories, and more.
          </Trans>
        }
      />
    );
  }

  return (
    <div
      className={cn(
        CARD_GRID,
        'gap-5 animate-in fade-in slide-in-from-bottom-4 duration-500',
      )}
    >
      {filters.map((filter, index) => (
        <div
          key={filter.id}
          style={{ animationDelay: `${index * 50}ms` }}
          className="h-full animate-in fade-in slide-in-from-bottom-2 fill-mode-backwards"
        >
          <FilterCard filter={filter} onEdit={onEdit} onDelete={onDelete} />
        </div>
      ))}
    </div>
  );
}
