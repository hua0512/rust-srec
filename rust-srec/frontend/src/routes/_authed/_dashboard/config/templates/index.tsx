import { createFileRoute, useNavigate } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { motion, AnimatePresence } from 'motion/react';
import { listTemplates } from '@/server/functions';
import { Trans } from '@lingui/react/macro';
import { t } from '@lingui/core/macro';
import { Skeleton } from '@/components/ui/skeleton';
import { TemplateCard } from '@/components/config/templates/template-card';
import { Badge } from '@/components/ui/badge';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { AlertCircle, Plus, LayoutTemplate, Search } from 'lucide-react';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { useState, useEffect, useMemo } from 'react';
import { Link } from '@tanstack/react-router';

export const Route = createFileRoute('/_authed/_dashboard/config/templates/')({
  component: TemplatesConfigPage,
});

function TemplatesConfigPage() {
  const navigate = useNavigate();
  const [searchQuery, setSearchQuery] = useState('');
  const [debouncedSearch, setDebouncedSearch] = useState('');

  useEffect(() => {
    const timer = setTimeout(() => {
      setDebouncedSearch(searchQuery);
    }, 300);
    return () => clearTimeout(timer);
  }, [searchQuery]);

  const {
    data: templates,
    isLoading,
    error,
  } = useQuery({
    queryKey: ['templates'],
    queryFn: () => listTemplates(),
  });

  const filteredTemplates = useMemo(() => {
    if (!templates) return [];
    if (!debouncedSearch) return templates;
    const search = debouncedSearch.toLowerCase();
    return templates.filter((t) => t.name.toLowerCase().includes(search));
  }, [templates, debouncedSearch]);

  const handleEdit = (templateId: string) => {
    navigate({ to: '/config/templates/$templateId', params: { templateId } });
  };

  if (error) {
    return (
      <Alert variant="destructive">
        <AlertCircle className="h-4 w-4" />
        <AlertTitle>
          <Trans>Error</Trans>
        </AlertTitle>
        <AlertDescription>
          <Trans>Failed to load templates: {(error as Error).message}</Trans>
        </AlertDescription>
      </Alert>
    );
  }

  return (
    <div className="space-y-6">
      {/* Search Bar and Create Button */}
      <div className="flex items-center gap-4">
        <div className="relative flex-1 max-w-sm">
          <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
          <Input
            placeholder={t`Search templates...`}
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="pl-9 h-9"
          />
        </div>
        <Badge
          variant="secondary"
          className="h-9 px-3 text-sm whitespace-nowrap"
        >
          {filteredTemplates.length} <Trans>templates</Trans>
        </Badge>
        <Link to="/config/templates/create">
          <Button className="gap-2">
            <Plus className="w-4 h-4" />
            <Trans>Create</Trans>
          </Button>
        </Link>
      </div>

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
              <div
                key={i}
                className="h-[220px] border rounded-xl bg-muted/10 animate-pulse flex flex-col p-6 space-y-4 shadow-sm"
              >
                <div className="flex justify-between items-start">
                  <Skeleton className="h-10 w-10 rounded-full" />
                  <Skeleton className="h-6 w-16" />
                </div>
                <div className="space-y-2 pt-2">
                  <Skeleton className="h-6 w-3/4" />
                  <Skeleton className="h-4 w-1/2" />
                </div>
                <div className="pt-4 mt-auto">
                  <Skeleton className="h-16 w-full rounded-md" />
                </div>
              </div>
            ))}
          </motion.div>
        ) : filteredTemplates.length > 0 ? (
          <motion.div
            key="list"
            className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4 gap-6"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            transition={{ duration: 0.3 }}
          >
            {filteredTemplates.map((template, index) => (
              <motion.div
                key={template.id}
                initial={{ opacity: 0, y: 20 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{
                  duration: 0.3,
                  delay: Math.min(index * 0.05, 0.3),
                }}
              >
                <TemplateCard
                  template={template}
                  onEdit={() => handleEdit(template.id)}
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
              <LayoutTemplate className="h-16 w-16 text-primary/60" />
            </div>
            <div className="space-y-2 max-w-md">
              <h3 className="font-semibold text-2xl tracking-tight">
                {debouncedSearch ? (
                  <Trans>No templates found</Trans>
                ) : (
                  <Trans>No templates yet</Trans>
                )}
              </h3>
              <p className="text-muted-foreground">
                {debouncedSearch ? (
                  <Trans>Try adjusting your search.</Trans>
                ) : (
                  <Trans>
                    Create a template to define reusable download
                    configurations.
                  </Trans>
                )}
              </p>
            </div>
            {!debouncedSearch && (
              <Link to="/config/templates/create">
                <Button className="gap-2">
                  <Plus className="w-4 h-4" />
                  <Trans>Create Template</Trans>
                </Button>
              </Link>
            )}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
