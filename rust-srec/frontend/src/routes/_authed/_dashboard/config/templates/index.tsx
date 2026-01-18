import { createFileRoute, useNavigate } from '@tanstack/react-router';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { motion, AnimatePresence } from 'motion/react';
import { listTemplates, cloneTemplate } from '@/server/functions';
import { Trans } from '@lingui/react/macro';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { Skeleton } from '@/components/ui/skeleton';
import { TemplateCard } from '@/components/config/templates/template-card';
import { Badge } from '@/components/ui/badge';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { AlertCircle, Plus, LayoutTemplate, Search } from 'lucide-react';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { useState, useEffect, useMemo } from 'react';
import { Link } from '@tanstack/react-router';
import { toast } from 'sonner';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Label } from '@/components/ui/label';
import { TemplateSchema } from '@/api/schemas';
import { z } from 'zod';

export const Route = createFileRoute('/_authed/_dashboard/config/templates/')({
  component: TemplatesConfigPage,
});

function TemplatesConfigPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { i18n } = useLingui();
  const [searchQuery, setSearchQuery] = useState('');
  const [debouncedSearch, setDebouncedSearch] = useState('');
  const [cloneDialogOpen, setCloneDialogOpen] = useState(false);
  const [templateToClone, setTemplateToClone] = useState<z.infer<
    typeof TemplateSchema
  > | null>(null);
  const [cloneName, setCloneName] = useState('');

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

  const cloneMutation = useMutation({
    mutationFn: cloneTemplate,
    onSuccess: (cloned) => {
      toast.success(i18n._(msg`Template cloned successfully`));
      queryClient.invalidateQueries({ queryKey: ['templates'] });
      setCloneDialogOpen(false);
      setTemplateToClone(null);
      setCloneName('');
      navigate({
        to: '/config/templates/$templateId',
        params: { templateId: cloned.id },
      });
    },
    onError: (error) =>
      toast.error(i18n._(msg`Failed to clone template: ${error.message}`)),
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

  const handleClone = (template: z.infer<typeof TemplateSchema>) => {
    setTemplateToClone(template);
    setCloneName(`${template.name}_copy`);
    setCloneDialogOpen(true);
  };

  const handleCloneConfirm = () => {
    if (templateToClone && cloneName.trim()) {
      cloneMutation.mutate({
        data: { id: templateToClone.id, new_name: cloneName.trim() },
      });
    }
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
            placeholder={i18n._(msg`Search templates...`)}
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
            className="grid grid-cols-1 lg:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4 gap-6"
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
            className="grid grid-cols-1 lg:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4 gap-6"
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
                  onClone={() => handleClone(template)}
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

      {/* Clone Dialog */}
      <Dialog open={cloneDialogOpen} onOpenChange={setCloneDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              <Trans>Clone Template</Trans>
            </DialogTitle>
            <DialogDescription>
              <Trans>
                Create a copy of "{templateToClone?.name}" with a new name.
              </Trans>
            </DialogDescription>
          </DialogHeader>
          <div className="grid gap-4 py-4">
            <div className="grid gap-2">
              <Label htmlFor="clone-name">
                <Trans>New Template Name</Trans>
              </Label>
              <Input
                id="clone-name"
                value={cloneName}
                onChange={(e) => setCloneName(e.target.value)}
                placeholder={i18n._(msg`Enter a unique name`)}
              />
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setCloneDialogOpen(false)}>
              <Trans>Cancel</Trans>
            </Button>
            <Button
              onClick={handleCloneConfirm}
              disabled={!cloneName.trim() || cloneMutation.isPending}
            >
              {cloneMutation.isPending ? (
                <Trans>Cloning...</Trans>
              ) : (
                <Trans>Clone</Trans>
              )}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
