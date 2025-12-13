import { createFileRoute, useNavigate } from '@tanstack/react-router';
import { useQuery, useMutation, useQueryClient, keepPreviousData } from '@tanstack/react-query';
import { motion, AnimatePresence } from 'motion/react';
import { z } from 'zod';
import { useState, useMemo, useRef, useEffect, useCallback } from 'react';
import {
    listJobPresets,
    deleteJobPreset,
    cloneJobPreset
} from '@/server/functions/job';
import { JobPresetSchema } from '@/api/schemas';

import { Button } from "@/components/ui/button";
import { Plus, Settings2, Search } from "lucide-react";
import { toast } from "sonner";
import { Trans } from "@lingui/react/macro";
import { t } from "@lingui/core/macro";
import { PresetCard } from '@/components/pipeline/presets/preset-card';
import { Skeleton } from "@/components/ui/skeleton";
import {
    Dialog,
    DialogContent,
    DialogDescription,
    DialogFooter,
    DialogHeader,
    DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { AlertCircle } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import {
    Select,
    SelectContent,
    SelectItem,
    SelectTrigger,
    SelectValue,
} from "@/components/ui/select";
import {
    Pagination,
    PaginationContent,
    PaginationEllipsis,
    PaginationItem,
    PaginationLink,
    PaginationNext,
    PaginationPrevious,
} from "@/components/ui/pagination";

export const Route = createFileRoute('/_authed/_dashboard/pipeline/presets/')({
    component: PresetsPage,
});

const CATEGORY_LABELS: Record<string, string> = {
    remux: "Remux",
    compression: "Compression",
    thumbnail: "Thumbnail",
    audio: "Audio",
    archive: "Archive",
    upload: "Upload",
    cleanup: "Cleanup",
    file_ops: "File Ops",
    custom: "Custom",
    metadata: "Metadata",
};

const PAGE_SIZES = [12, 24, 48, 96];

function PresetsPage() {
    const navigate = useNavigate();
    const queryClient = useQueryClient();
    const [selectedCategory, setSelectedCategory] = useState<string | null>(null);
    const [searchQuery, setSearchQuery] = useState('');
    const [debouncedSearch, setDebouncedSearch] = useState('');
    const [pageSize, setPageSize] = useState(24);
    const [currentPage, setCurrentPage] = useState(0);
    const [cloneDialogOpen, setCloneDialogOpen] = useState(false);
    const [presetToClone, setPresetToClone] = useState<z.infer<typeof JobPresetSchema> | null>(null);
    const [cloneName, setCloneName] = useState('');

    // Proper debounce with useEffect
    useEffect(() => {
        const timer = setTimeout(() => {
            setDebouncedSearch(searchQuery);
            setCurrentPage(0);
        }, 300);
        return () => clearTimeout(timer);
    }, [searchQuery]);

    // Reset page when category changes
    const handleCategoryChange = useCallback((category: string | null) => {
        setSelectedCategory(category);
        setCurrentPage(0);
    }, []);

    // Server-side filtered query with pagination
    const { data, isLoading, isError, error } = useQuery({
        queryKey: ['job', 'presets', selectedCategory, debouncedSearch, pageSize, currentPage],
        queryFn: () => listJobPresets({
            data: {
                category: selectedCategory || undefined,
                search: debouncedSearch || undefined,
                limit: pageSize,
                offset: currentPage * pageSize,
            }
        }),
        staleTime: 30000, // 30 seconds
        placeholderData: keepPreviousData,
    });

    const presets = data?.presets;
    const totalCount = data?.total ?? 0;
    const totalPages = Math.ceil(totalCount / pageSize);

    // Memoize categories to prevent flickering
    const categoriesRef = useRef<string[]>([]);
    const categories = useMemo(() => {
        if (data?.categories && data.categories.length > 0) {
            categoriesRef.current = data.categories;
        }
        return categoriesRef.current;
    }, [data?.categories]);

    // Memoize pagination pages calculation
    const paginationPages = useMemo(() => {
        const pages: (number | 'ellipsis')[] = [];
        if (totalPages <= 7) {
            for (let i = 0; i < totalPages; i++) pages.push(i);
        } else {
            pages.push(0);
            if (currentPage > 2) pages.push('ellipsis');
            for (let i = Math.max(1, currentPage - 1); i <= Math.min(totalPages - 2, currentPage + 1); i++) {
                pages.push(i);
            }
            if (currentPage < totalPages - 3) pages.push('ellipsis');
            pages.push(totalPages - 1);
        }
        return pages;
    }, [totalPages, currentPage]);

    const deleteMutation = useMutation({
        mutationFn: deleteJobPreset,
        onSuccess: () => {
            toast.success(t`Preset deleted successfully`);
            queryClient.invalidateQueries({ queryKey: ['job', 'presets'] });
        },
        onError: (error) => toast.error(t`Failed to delete preset: ${error.message}`),
    });

    const cloneMutation = useMutation({
        mutationFn: cloneJobPreset,
        onSuccess: (cloned) => {
            toast.success(t`Preset cloned successfully`);
            queryClient.invalidateQueries({ queryKey: ['job', 'presets'] });
            setCloneDialogOpen(false);
            setPresetToClone(null);
            setCloneName('');
            // Navigate to edit the cloned preset
            navigate({ to: '/pipeline/presets/$presetId', params: { presetId: cloned.id } });
        },
        onError: (error) => toast.error(t`Failed to clone preset: ${error.message}`),
    });

    const handleDelete = (id: string) => {
        deleteMutation.mutate({ data: id });
    };

    const handleEdit = (preset: z.infer<typeof JobPresetSchema>) => {
        navigate({ to: '/pipeline/presets/$presetId', params: { presetId: preset.id } });
    };

    const handleClone = (preset: z.infer<typeof JobPresetSchema>) => {
        setPresetToClone(preset);
        setCloneName(`${preset.name}_copy`);
        setCloneDialogOpen(true);
    };

    const handleCloneConfirm = () => {
        if (presetToClone && cloneName.trim()) {
            cloneMutation.mutate({ data: { id: presetToClone.id, new_name: cloneName.trim() } });
        }
    };

    const handleCreate = () => {
        navigate({ to: '/pipeline/presets/create' });
    };

    // Show error if fetch failed
    if (isError) {
        return (
            <div className="space-y-8 p-6 md:p-10 max-w-7xl mx-auto">
                <Alert variant="destructive">
                    <AlertCircle className="h-4 w-4" />
                    <AlertTitle><Trans>Error</Trans></AlertTitle>
                    <AlertDescription>
                        <Trans>Failed to load presets: {error.message}</Trans>
                    </AlertDescription>
                </Alert>
            </div>
        );
    }

    return (
        <div className="min-h-screen space-y-6">
            {/* Header */}
            <div className="border-b border-border/40">
                <div className="w-full">
                    {/* Title Row */}
                    <div className="flex flex-col md:flex-row gap-4 items-start md:items-center justify-between p-4 md:px-8">
                        <div className="flex items-center gap-4">
                            <div className="p-2.5 rounded-xl bg-gradient-to-br from-primary/20 to-primary/5 ring-1 ring-primary/10">
                                <Settings2 className="h-6 w-6 text-primary" />
                            </div>
                            <div>
                                <h1 className="text-xl font-semibold tracking-tight"><Trans>Presets</Trans></h1>
                                <p className="text-sm text-muted-foreground">
                                    <Trans>Reusable processor configurations for pipeline steps</Trans>
                                </p>
                            </div>
                        </div>
                        <div className="flex items-center gap-2 w-full md:w-auto">
                            {/* Search Input */}
                            <div className="relative flex-1 md:w-64">
                                <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
                                <Input
                                    placeholder={t`Search presets...`}
                                    value={searchQuery}
                                    onChange={(e) => setSearchQuery(e.target.value)}
                                    className="pl-9 h-9"
                                />
                            </div>
                            <Badge variant="secondary" className="h-9 px-3 text-sm whitespace-nowrap">
                                {totalCount} <Trans>presets</Trans>
                            </Badge>
                        </div>
                    </div>

                    {/* Category Filter */}
                    <div className="px-4 md:px-8 pb-3 overflow-x-auto no-scrollbar">
                        <nav className="flex items-center gap-1">
                            <button
                                onClick={() => handleCategoryChange(null)}
                                className={`relative px-3 py-1.5 text-sm font-medium rounded-full transition-all duration-200 ${selectedCategory === null
                                    ? "bg-primary text-primary-foreground shadow-sm"
                                    : "text-muted-foreground hover:text-foreground hover:bg-muted"
                                    }`}
                            >
                                <span className="relative z-10 flex items-center gap-1.5">
                                    <Trans>All</Trans>
                                </span>
                            </button>

                            {categories.map(cat => (
                                <button
                                    key={cat}
                                    onClick={() => handleCategoryChange(cat)}
                                    className={`relative px-3 py-1.5 text-sm font-medium rounded-full transition-all duration-200 ${selectedCategory === cat
                                        ? "bg-primary text-primary-foreground shadow-sm"
                                        : "text-muted-foreground hover:text-foreground hover:bg-muted"
                                        }`}
                                >
                                    <span className="relative z-10 capitalize">
                                        {CATEGORY_LABELS[cat] || cat}
                                    </span>
                                </button>
                            ))}
                        </nav>
                    </div>
                </div>
            </div>

            <div className="p-4 md:px-8 pb-20 w-full">
                <AnimatePresence mode="wait">
                    {isLoading ? (
                        <motion.div
                            key="loading"
                            initial={{ opacity: 0 }}
                            animate={{ opacity: 1 }}
                            exit={{ opacity: 0 }}
                            className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-6"
                        >
                            {[1, 2, 3, 4].map((i) => (
                                <div key={i} className="h-[220px] border rounded-xl bg-muted/10 animate-pulse flex flex-col p-6 space-y-4 shadow-sm">
                                    <div className="flex justify-between items-start">
                                        <Skeleton className="h-10 w-10 rounded-full" />
                                        <Skeleton className="h-6 w-8" />
                                    </div>
                                    <div className="space-y-2 pt-2">
                                        <Skeleton className="h-6 w-3/4" />
                                        <Skeleton className="h-4 w-1/2" />
                                    </div>
                                    <div className="pt-4 mt-auto">
                                        <Skeleton className="h-20 w-full rounded-md" />
                                    </div>
                                </div>
                            ))}
                        </motion.div>
                    ) : presets && presets.length > 0 ? (
                        <motion.div
                            key="list"
                            className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-6"
                            initial={{ opacity: 0 }}
                            animate={{ opacity: 1 }}
                            transition={{ duration: 0.3 }}
                        >
                            {presets.map((preset, index) => (
                                <motion.div
                                    key={preset.id}
                                    initial={{ opacity: 0, y: 20 }}
                                    animate={{ opacity: 1, y: 0 }}
                                    transition={{
                                        duration: 0.3,
                                        delay: Math.min(index * 0.05, 0.3) // Cap delay at 0.3s
                                    }}
                                >
                                    <PresetCard
                                        preset={preset}
                                        onEdit={handleEdit}
                                        onDelete={handleDelete}
                                        onClone={handleClone}
                                    />
                                </motion.div>
                            ))}
                        </motion.div>
                    ) : (
                        <motion.div
                            key="empty"
                            initial={{ opacity: 0, scale: 0.95 }}
                            animate={{ opacity: 1, scale: 1 }}
                            className="flex flex-col items-center justify-center py-32 text-center space-y-6 border-2 border-dashed border-muted-foreground/20 rounded-2xl bg-muted/5 backdrop-blur-sm shadow-sm"
                        >
                            <div className="p-6 bg-primary/5 rounded-full ring-1 ring-primary/10">
                                <Settings2 className="h-16 w-16 text-primary/60" />
                            </div>
                            <div className="space-y-2 max-w-md">
                                <h3 className="font-semibold text-2xl tracking-tight">
                                    {debouncedSearch ? <Trans>No presets found</Trans> : <Trans>No presets yet</Trans>}
                                </h3>
                                <p className="text-muted-foreground">
                                    {debouncedSearch
                                        ? <Trans>Try adjusting your search or filters.</Trans>
                                        : <Trans>Create your first job preset to define reusable processor configurations for your pipelines.</Trans>
                                    }
                                </p>
                            </div>
                            {!debouncedSearch && (
                                <Button onClick={handleCreate} size="lg" className="mt-4">
                                    <Plus className="mr-2 h-5 w-5" />
                                    <Trans>Create Preset</Trans>
                                </Button>
                            )}
                        </motion.div>
                    )}
                </AnimatePresence>

                {/* Pagination Controls */}
                {totalPages > 1 && (
                    <div className="flex items-center justify-between mt-8 pt-6 border-t">
                        <div className="flex items-center gap-2">
                            <span className="text-sm text-muted-foreground"><Trans>Per page:</Trans></span>
                            <Select
                                value={pageSize.toString()}
                                onValueChange={(value) => {
                                    setPageSize(Number(value));
                                    setCurrentPage(0);
                                }}
                            >
                                <SelectTrigger className="w-20 h-8">
                                    <SelectValue />
                                </SelectTrigger>
                                <SelectContent>
                                    {PAGE_SIZES.map((size) => (
                                        <SelectItem key={size} value={size.toString()}>
                                            {size}
                                        </SelectItem>
                                    ))}
                                </SelectContent>
                            </Select>
                        </div>

                        <Pagination>
                            <PaginationContent>
                                <PaginationItem>
                                    <PaginationPrevious
                                        onClick={() => setCurrentPage(p => Math.max(0, p - 1))}
                                        className={currentPage === 0 ? "pointer-events-none opacity-50" : "cursor-pointer"}
                                    />
                                </PaginationItem>

                                {paginationPages.map((page, idx) =>
                                    page === 'ellipsis' ? (
                                        <PaginationItem key={`ellipsis-${idx}`}>
                                            <PaginationEllipsis />
                                        </PaginationItem>
                                    ) : (
                                        <PaginationItem key={page}>
                                            <PaginationLink
                                                isActive={currentPage === page}
                                                onClick={() => setCurrentPage(page)}
                                                className="cursor-pointer"
                                            >
                                                {page + 1}
                                            </PaginationLink>
                                        </PaginationItem>
                                    )
                                )}

                                <PaginationItem>
                                    <PaginationNext
                                        onClick={() => setCurrentPage(p => Math.min(totalPages - 1, p + 1))}
                                        className={currentPage >= totalPages - 1 ? "pointer-events-none opacity-50" : "cursor-pointer"}
                                    />
                                </PaginationItem>
                            </PaginationContent>
                        </Pagination>
                    </div>
                )}

                {/* Clone Dialog */}
                <Dialog open={cloneDialogOpen} onOpenChange={setCloneDialogOpen}>
                    <DialogContent>
                        <DialogHeader>
                            <DialogTitle><Trans>Clone Preset</Trans></DialogTitle>
                            <DialogDescription>
                                <Trans>Create a copy of "{presetToClone?.name}" with a new name.</Trans>
                            </DialogDescription>
                        </DialogHeader>
                        <div className="grid gap-4 py-4">
                            <div className="grid gap-2">
                                <Label htmlFor="clone-name"><Trans>New Preset Name</Trans></Label>
                                <Input
                                    id="clone-name"
                                    value={cloneName}
                                    onChange={(e) => setCloneName(e.target.value)}
                                    placeholder={t`Enter a unique name`}
                                />
                            </div>
                        </div>
                        <DialogFooter>
                            <Button variant="outline" onClick={() => setCloneDialogOpen(false)}>
                                <Trans>Cancel</Trans>
                            </Button>
                            <Button onClick={handleCloneConfirm} disabled={!cloneName.trim() || cloneMutation.isPending}>
                                {cloneMutation.isPending ? <Trans>Cloning...</Trans> : <Trans>Clone</Trans>}
                            </Button>
                        </DialogFooter>
                    </DialogContent>
                </Dialog>

                {/* Floating Action Button */}
                <motion.div
                    className="fixed bottom-8 right-8 z-50"
                    initial={{ scale: 0, rotate: 90 }}
                    animate={{ scale: 1, rotate: 0 }}
                    whileHover={{ scale: 1.1 }}
                    whileTap={{ scale: 0.9 }}
                >
                    <Button
                        onClick={handleCreate}
                        size="icon"
                        className="h-14 w-14 rounded-full shadow-2xl bg-primary hover:bg-primary/90 text-primary-foreground flex items-center justify-center p-0"
                    >
                        <Plus className="h-6 w-6" />
                    </Button>
                </motion.div>
            </div>
        </div>
    );
}
