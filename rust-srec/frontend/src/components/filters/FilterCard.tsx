import { FilterSchema, TimeBasedFilterConfigSchema, KeywordFilterConfigSchema, CategoryFilterConfigSchema, CronFilterConfigSchema, RegexFilterConfigSchema } from '../../api/schemas';
import { z } from 'zod';
import { Card, CardContent, CardHeader } from '../ui/card';
import { Badge } from '../ui/badge';
import { Button } from '../ui/button';
import { Edit, Trash2, Clock, Tag, Folder, Calendar } from 'lucide-react';
import { Trans } from '@lingui/macro';

type Filter = z.infer<typeof FilterSchema>;

interface FilterCardProps {
    filter: Filter;
    onEdit: (filter: Filter) => void;
    onDelete: (filterId: string) => void;
}

const TYPE_COLORS: Record<string, string> = {
    TIME_BASED: "border-t-blue-500",
    KEYWORD: "border-t-emerald-500",
    CATEGORY: "border-t-violet-500",
    CRON: "border-t-orange-500",
    REGEX: "border-t-pink-500",
};

const TYPE_BG_COLORS: Record<string, string> = {
    TIME_BASED: "bg-blue-500/10 text-blue-700 dark:text-blue-300",
    KEYWORD: "bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
    CATEGORY: "bg-violet-500/10 text-violet-700 dark:text-violet-300",
    CRON: "bg-orange-500/10 text-orange-700 dark:text-orange-300",
    REGEX: "bg-pink-500/10 text-pink-700 dark:text-pink-300",
};

export function FilterCard({ filter, onEdit, onDelete }: FilterCardProps) {
    const borderColor = TYPE_COLORS[filter.filter_type] || "border-t-gray-500";
    const headerStyle = TYPE_BG_COLORS[filter.filter_type] || "bg-gray-500/10";

    const renderConfig = () => {
        switch (filter.filter_type) {
            case 'TIME_BASED': {
                const config = TimeBasedFilterConfigSchema.safeParse(filter.config);
                if (!config.success) return <span className="text-destructive text-xs">Invalid Config</span>;
                const { days, start_time, end_time } = config.data;
                const allDays = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];

                return (
                    <div className="space-y-3">
                        <div className="flex gap-1">
                            {allDays.map(d => {
                                const isActive = days.includes(d);
                                return (
                                    <div key={d} className={`w-5 h-5 rounded-full flex items-center justify-center text-[10px] font-bold transition-colors ${isActive ? 'bg-primary text-primary-foreground' : 'bg-muted text-muted-foreground opacity-50'}`}>
                                        {d[0]}
                                    </div>
                                );
                            })}
                        </div>
                        <div className="flex items-center text-sm font-mono text-muted-foreground bg-muted/40 px-2 py-1 rounded-md border w-fit">
                            <Clock className="w-3.5 h-3.5 mr-2 text-primary" />
                            {start_time.slice(0, 5)} - {end_time.slice(0, 5)}
                        </div>
                    </div>
                );
            }
            case 'KEYWORD': {
                const config = KeywordFilterConfigSchema.safeParse(filter.config);
                if (!config.success) return <span className="text-destructive text-xs">Invalid Config</span>;
                const hasKeywords = config.data.keywords.length > 0;

                return (
                    <div className="space-y-3">
                        <div className="flex gap-2 items-center">
                            <Badge variant={config.data.exclude ? "destructive" : "default"} className="text-[10px] px-2 h-5 shadow-none rounded-md">
                                {config.data.exclude ? <Trans>Exclude</Trans> : <Trans>Include</Trans>}
                            </Badge>
                            {config.data.case_sensitive && (
                                <Badge variant="outline" className="text-[10px] px-2 h-5 bg-background text-muted-foreground"><Trans>Aa</Trans></Badge>
                            )}
                        </div>

                        {hasKeywords ? (
                            <div className="flex gap-1.5 flex-wrap">
                                {config.data.keywords.map(k => (
                                    <Badge key={k} variant="secondary" className="px-1.5 py-0 h-5 text-[11px] font-normal border bg-background hover:bg-muted transition-colors">
                                        {k}
                                    </Badge>
                                ))}
                            </div>
                        ) : (
                            <div className="text-xs text-muted-foreground italic pl-1 flex items-center gap-1 opacity-70">
                                <Tag className="w-3 h-3" /> <Trans>No keywords set</Trans>
                            </div>
                        )}
                    </div>
                );
            }
            case 'CATEGORY': {
                const config = CategoryFilterConfigSchema.safeParse(filter.config);
                if (!config.success) return <span className="text-destructive text-xs">Invalid Config</span>;
                const hasCategories = config.data.categories.length > 0;

                return (
                    <div className="space-y-3">
                        <div className="flex gap-2 items-center">
                            <Badge variant={config.data.exclude ? "destructive" : "default"} className="text-[10px] px-2 h-5 shadow-none rounded-md">
                                {config.data.exclude ? <Trans>Exclude</Trans> : <Trans>Include</Trans>}
                            </Badge>
                        </div>
                        {hasCategories ? (
                            <div className="flex gap-1.5 flex-wrap">
                                {config.data.categories.map(c => (
                                    <Badge key={c} variant="secondary" className="px-1.5 py-0 h-5 text-[11px] font-normal border-transparent bg-violet-100 text-violet-800 hover:bg-violet-200 dark:bg-violet-900/30 dark:text-violet-300">
                                        {c}
                                    </Badge>
                                ))}
                            </div>
                        ) : (
                            <div className="text-xs text-muted-foreground italic pl-1 flex items-center gap-1 opacity-70">
                                <Folder className="w-3 h-3" /> <Trans>No categories set</Trans>
                            </div>
                        )}
                    </div>
                );
            }
            case 'CRON': {
                const config = CronFilterConfigSchema.safeParse(filter.config);
                if (!config.success) return <span className="text-destructive text-xs">Invalid Config</span>;
                return (
                    <div className="space-y-2">
                        <div className="bg-muted/50 p-2 rounded-lg border flex items-center justify-between group-hover:border-primary/20 transition-colors">
                            <code className="text-sm font-bold text-primary font-mono tracking-tight">{config.data.expression}</code>
                        </div>
                        {config.data.timezone && (
                            <div className="flex items-center text-[10px] text-muted-foreground px-1">
                                <span className="w-1.5 h-1.5 rounded-full bg-emerald-500 mr-2 animate-pulse"></span>
                                {config.data.timezone}
                            </div>
                        )}
                    </div>
                );
            }
            case 'REGEX': {
                const config = RegexFilterConfigSchema.safeParse(filter.config);
                if (!config.success) return <span className="text-destructive text-xs">Invalid Config</span>;
                return (
                    <div className="space-y-3">
                        <div className="flex gap-2 items-center">
                            <Badge variant={config.data.exclude ? "destructive" : "default"} className="text-[10px] px-2 h-5 shadow-none rounded-md">
                                {config.data.exclude ? <Trans>Exclude</Trans> : <Trans>Include</Trans>}
                            </Badge>
                            {config.data.case_insensitive && <Badge variant="outline" className="text-[10px] px-2 h-5 bg-background text-muted-foreground"><Trans>i</Trans></Badge>}
                        </div>
                        <div className="bg-slate-950 text-slate-50 px-3 py-2 rounded-lg border shadow-sm font-mono text-[11px] break-all leading-relaxed">
                            {config.data.pattern || <span className="opacity-50 italic">Empty pattern</span>}
                        </div>
                    </div>
                );
            }
            default:
                return <span className="text-muted-foreground">Unknown Filter Type</span>;
        }
    };

    const FilterIcon = () => {
        switch (filter.filter_type) {
            case 'TIME_BASED': return <Clock className="w-3.5 h-3.5" />;
            case 'KEYWORD': return <Tag className="w-3.5 h-3.5" />;
            case 'CATEGORY': return <Folder className="w-3.5 h-3.5" />;
            case 'CRON': return <Calendar className="w-3.5 h-3.5" />;
            case 'REGEX': return <Tag className="w-3.5 h-3.5" />;
            default: return <Tag className="w-3.5 h-3.5" />;
        }
    }

    return (
        <Card className={`group relative overflow-hidden border-t-4 transition-all hover:shadow-lg hover:-translate-y-1 ${borderColor}`}>
            <CardHeader className="pb-3 pt-4">
                <div className="flex items-center justify-between">
                    <div className={`flex items-center gap-2 px-2.5 py-1 rounded-full text-xs font-semibold ${headerStyle}`}>
                        <FilterIcon />
                        {filter.filter_type.replace('_', ' ')}
                    </div>

                    <div className="flex gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                        <Button variant="ghost" size="icon" className="h-7 w-7 rounded-full hover:bg-muted" onClick={() => onEdit(filter)}>
                            <Edit className="h-3.5 w-3.5" />
                            <span className="sr-only"><Trans>Edit</Trans></span>
                        </Button>
                        <Button variant="ghost" size="icon" className="h-7 w-7 rounded-full text-muted-foreground hover:text-destructive hover:bg-destructive/10" onClick={() => onDelete(filter.id)}>
                            <Trash2 className="h-3.5 w-3.5" />
                            <span className="sr-only"><Trans>Delete</Trans></span>
                        </Button>
                    </div>
                </div>
            </CardHeader>
            <CardContent className="pt-2">
                {renderConfig()}
            </CardContent>
        </Card>
    );
}
