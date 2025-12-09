import { Input } from '../ui/input';
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from '../ui/select';
import { Button } from '../ui/button';
import { Search, Plus, Filter, X } from 'lucide-react';
import { Link } from '@tanstack/react-router';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';

interface StreamersToolbarProps {
    search: string;
    onSearchChange: (value: string) => void;
    platformFilter: string;
    onPlatformFilterChange: (value: string) => void;
    stateFilter: string;
    onStateFilterChange: (value: string) => void;
    platforms: { id: string; name: string }[];
    onResetFilters: () => void;
}

export function StreamersToolbar({
    search,
    onSearchChange,
    platformFilter,
    onPlatformFilterChange,
    stateFilter,
    onStateFilterChange,
    platforms,
    onResetFilters,
}: StreamersToolbarProps) {
    const states = [
        { value: 'LIVE', label: t`Live` },
        { value: 'NOT_LIVE', label: t`Offline` },
        { value: 'ERROR', label: t`Error` },
        { value: 'DISABLED', label: t`Disabled` },
    ];

    const hasActiveFilters = platformFilter !== 'all' || stateFilter !== 'all' || search !== '';

    return (
        <div className="flex flex-col sm:flex-row gap-4 items-start sm:items-center justify-between py-4">
            <div className="flex flex-1 items-center gap-3 w-full sm:max-w-xl">
                <div className="relative flex-1">
                    <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
                    <Input
                        type="search"
                        placeholder={t`Search streamers...`}
                        className="pl-9 bg-background/50 border-muted-foreground/20 focus:bg-background transition-colors"
                        value={search}
                        onChange={(e) => onSearchChange(e.target.value)}
                    />
                </div>

                {/* Platform Filter */}
                <Select value={platformFilter} onValueChange={onPlatformFilterChange}>
                    <SelectTrigger className="w-[160px] bg-background/50 border-muted-foreground/20">
                        <div className="flex items-center gap-2">
                            <Filter className="h-3.5 w-3.5 text-muted-foreground/70" />
                            <SelectValue placeholder={t`Platform`} />
                        </div>
                    </SelectTrigger>
                    <SelectContent>
                        <SelectItem value="all"><Trans>All Platforms</Trans></SelectItem>
                        {platforms.map((p) => (
                            <SelectItem key={p.id} value={p.id}>
                                {p.name}
                            </SelectItem>
                        ))}
                    </SelectContent>
                </Select>

                {/* State Filter */}
                <Select value={stateFilter} onValueChange={onStateFilterChange}>
                    <SelectTrigger className="w-[140px] bg-background/50 border-muted-foreground/20">
                        <div className="flex items-center gap-2">
                            <SelectValue placeholder={t`State`} />
                        </div>
                    </SelectTrigger>
                    <SelectContent>
                        <SelectItem value="all"><Trans>All States</Trans></SelectItem>
                        {states.map((s) => (
                            <SelectItem key={s.value} value={s.value}>
                                {s.label}
                            </SelectItem>
                        ))}
                    </SelectContent>
                </Select>

                {hasActiveFilters && (
                    <Button variant="ghost" size="icon" onClick={onResetFilters} title={t`Reset filters`} className="text-muted-foreground hover:text-foreground">
                        <X className="h-4 w-4" />
                    </Button>
                )}
            </div>

            <Button asChild className="space-x-2">
                <Link to="/streamers/new">
                    <Plus className="h-4 w-4 mr-2" /> <Trans>Add Streamer</Trans>
                </Link>
            </Button>
        </div>
    );
}
