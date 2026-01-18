import { FilterSchema } from '../../api/schemas';
import { z } from 'zod';
import { FilterCard } from './FilterCard';
import { Trans } from '@lingui/react/macro';
import { FilterX } from 'lucide-react';

type Filter = z.infer<typeof FilterSchema>;

interface FilterListProps {
  filters: Filter[];
  onEdit: (filter: Filter) => void;
  onDelete: (filterId: string) => void;
}

export function FilterList({ filters, onEdit, onDelete }: FilterListProps) {
  if (filters.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center py-12 px-4 border-2 border-dashed rounded-xl bg-muted/30 text-center animate-in fade-in zoom-in-95 duration-300">
        <div className="bg-background p-3 rounded-full shadow-sm mb-4">
          <FilterX className="w-8 h-8 text-muted-foreground" />
        </div>
        <h3 className="text-lg font-semibold">
          <Trans>No filters yet</Trans>
        </h3>
        <p className="text-muted-foreground max-w-sm mt-1 mb-4">
          <Trans>
            Create a filter to control when this streamer is recorded. You can
            filter by time, keywords, categories, and more.
          </Trans>
        </p>
      </div>
    );
  }

  return (
    <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-5 animate-in fade-in slide-in-from-bottom-4 duration-500">
      {filters.map((filter, index) => (
        <div
          key={filter.id}
          style={{ animationDelay: `${index * 50}ms` }}
          className="animate-in fade-in slide-in-from-bottom-2 fill-mode-backwards"
        >
          <FilterCard filter={filter} onEdit={onEdit} onDelete={onDelete} />
        </div>
      ))}
    </div>
  );
}
