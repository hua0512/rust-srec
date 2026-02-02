import { useState } from 'react';
import { Plus } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Trans } from '@lingui/react/macro';
import { Skeleton } from '@/components/ui/skeleton';
import { FilterList } from '../../filters/FilterList';
import { FilterDialog } from '../../filters/FilterDialog';

interface StreamerFiltersTabProps {
  streamerId: string;
  filters: any[];
  isLoading: boolean;
  onDeleteFilter: (filterId: string) => void;
}

export function StreamerFiltersTab({
  streamerId,
  filters,
  isLoading,
  onDeleteFilter,
}: StreamerFiltersTabProps) {
  const [filterDialogOpen, setFilterDialogOpen] = useState(false);
  const [filterToEdit, setFilterToEdit] = useState<any>(null);

  return (
    <div className="space-y-4">
      <div className="flex flex-col sm:flex-row justify-between items-start sm:items-center bg-card p-4 rounded-lg border shadow-sm gap-4">
        <div className="space-y-1">
          <h3 className="text-lg font-medium">
            <Trans>Recording Filters</Trans>
          </h3>
          <p className="text-sm text-muted-foreground">
            <Trans>
              Define rules to automatically record or skip streams based on
              title, time, or language.
            </Trans>
          </p>
        </div>
        <Button
          size="sm"
          onClick={() => {
            setFilterToEdit(null);
            setFilterDialogOpen(true);
          }}
        >
          <Plus className="mr-2 h-4 w-4" /> <Trans>Add Filter</Trans>
        </Button>
      </div>

      {isLoading ? (
        <div className="grid grid-cols-1 md:grid-cols-3 gap-6">
          <Skeleton className="h-40 w-full rounded-xl" />
          <Skeleton className="h-40 w-full rounded-xl" />
          <Skeleton className="h-40 w-full rounded-xl" />
        </div>
      ) : (
        <div className="min-h-[200px]">
          <FilterList
            filters={filters || []}
            onEdit={(filter) => {
              setFilterToEdit(filter);
              setFilterDialogOpen(true);
            }}
            onDelete={onDeleteFilter}
          />
        </div>
      )}

      <FilterDialog
        streamerId={streamerId}
        open={filterDialogOpen}
        onOpenChange={setFilterDialogOpen}
        filterToEdit={filterToEdit}
      />
    </div>
  );
}
