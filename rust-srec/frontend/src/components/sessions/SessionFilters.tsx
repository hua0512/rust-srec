import { Input } from '../ui/input';
import { Button } from '../ui/button';
import { Search, CalendarIcon, X } from 'lucide-react';
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from "../ui/select"
import { Popover, PopoverContent, PopoverTrigger } from '../ui/popover';
import { Calendar } from '../ui/calendar';
import { cn } from '@/lib/utils';
import { useState } from 'react';
import { DateRange } from 'react-day-picker';
import { format } from 'date-fns';

interface SessionFiltersProps {
    search: string;
    onSearchChange: (val: string) => void;
    status: string;
    onStatusChange: (val: string) => void;
    timeRange: string;
    onTimeRangeChange: (val: string) => void;
    dateRange?: DateRange;
    onDateRangeChange?: (range: DateRange | undefined) => void;
    onClear: () => void;
}

export function SessionFilters({
    search,
    onSearchChange,
    status,
    onStatusChange,
    timeRange,
    onTimeRangeChange,
    dateRange,
    onDateRangeChange,
    onClear
}: SessionFiltersProps) {
    const [isCalendarOpen, setIsCalendarOpen] = useState(false);

    const hasActiveFilters = search || status !== 'all' || timeRange !== 'all' || (dateRange?.from);

    return (
        <div className="flex flex-col gap-4 bg-card/30 p-4 rounded-xl backdrop-blur-xs">
            <div className="flex flex-col sm:flex-row gap-3 items-center">
                <div className="relative flex-1 w-full">
                    <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
                    <Input
                        placeholder="Search sessions by title or streamer..."
                        className="pl-9 bg-background/50 border-input/50 focus-visible:ring-1"
                        value={search}
                        onChange={(e) => onSearchChange(e.target.value)}
                    />
                </div>

                <div className="flex items-center gap-2 w-full sm:w-auto overflow-x-auto sm:overflow-visible pb-1 sm:pb-0">
                    <Select value={status} onValueChange={onStatusChange}>
                        <SelectTrigger className="w-[140px] bg-background/50 border-input/50">
                            <SelectValue placeholder="Status" />
                        </SelectTrigger>
                        <SelectContent>
                            <SelectItem value="all">All Status</SelectItem>
                            <SelectItem value="active">Live & Active</SelectItem>
                            <SelectItem value="completed">Completed</SelectItem>
                        </SelectContent>
                    </Select>

                    <Popover open={isCalendarOpen} onOpenChange={setIsCalendarOpen}>
                        <PopoverTrigger asChild>
                            <Button
                                variant={"outline"}
                                className={cn(
                                    "w-[240px] justify-start text-left font-normal bg-background/50 border-input/50 truncate",
                                    (!timeRange || timeRange === 'all') && !dateRange?.from && "text-muted-foreground"
                                )}
                            >
                                <CalendarIcon className="mr-2 h-4 w-4 opacity-50 shrink-0" />
                                {dateRange?.from ? (
                                    dateRange.to ? (
                                        <>
                                            {format(dateRange.from, "LLL dd, y")} -{" "}
                                            {format(dateRange.to, "LLL dd, y")}
                                        </>
                                    ) : (
                                        format(dateRange.from, "LLL dd, y")
                                    )
                                ) : timeRange === 'all' ? (
                                    <span>Pick a date range</span>
                                ) : (
                                    <span className="capitalize">{timeRange}</span>
                                )}
                            </Button>
                        </PopoverTrigger>
                        <PopoverContent className="w-auto p-0" align="end">
                            <div className="flex flex-col">
                                <div className="p-3 border-b">
                                    <Select value={timeRange} onValueChange={(val) => {
                                        onTimeRangeChange(val);
                                        if (val !== 'custom') setIsCalendarOpen(false);
                                    }}>
                                        <SelectTrigger className="w-full">
                                            <SelectValue placeholder="Select preset..." />
                                        </SelectTrigger>
                                        <SelectContent position="popper">
                                            <SelectItem value="all">All Time</SelectItem>
                                            <SelectItem value="today">Today</SelectItem>
                                            <SelectItem value="yesterday">Yesterday</SelectItem>
                                            <SelectItem value="week">This Week</SelectItem>
                                            <SelectItem value="month">This Month</SelectItem>
                                            <SelectItem value="custom">Custom Range</SelectItem>
                                        </SelectContent>
                                    </Select>
                                </div>
                                <div className="p-3">
                                    <Calendar
                                        mode="range"
                                        selected={dateRange}
                                        onSelect={onDateRangeChange}
                                        disabled={(date) =>
                                            date > new Date() || date < new Date("1900-01-01")
                                        }
                                        numberOfMonths={2}
                                        initialFocus
                                        className="rounded-md border shadow-xs"
                                    />
                                </div>
                            </div>
                        </PopoverContent>
                    </Popover>

                    {hasActiveFilters && (
                        <Button
                            variant="ghost"
                            size="icon"
                            onClick={onClear}
                            title="Clear filters"
                            className="text-muted-foreground hover:text-destructive hover:bg-destructive/10 transition-colors"
                        >
                            <X className="h-4 w-4" />
                        </Button>
                    )}
                </div>
            </div>
        </div>
    );
}
