import { Handle, Position, NodeProps, type Node } from '@xyflow/react';
import { cn } from '@/lib/utils';
import { DagStepDefinition } from '@/api/schemas';
import { Settings2, Trash2, Database, Zap, Box } from 'lucide-react';
import { Button } from '@/components/ui/button';

import { GlassNode } from '../../graph-shared';

export type StepNodeData = {
  step: DagStepDefinition['step'];
  id: string;
  onEdit?: (id: string) => void;
  onRemove?: (id: string) => void;
};

export type StepNode = Node<StepNodeData, 'stepNode'>;

export function StepNode({ data }: NodeProps<StepNode>) {
  const { step, id, onEdit, onRemove } = data;

  const isPreset = step.type === 'preset';
  const isWorkflow = step.type === 'workflow';

  const Icon = isPreset ? Database : isWorkflow ? Zap : Box;
  const colorClass = isPreset
    ? 'text-blue-500'
    : isWorkflow
      ? 'text-purple-500'
      : 'text-emerald-500';
  const bgClass = isPreset
    ? 'bg-blue-500/10'
    : isWorkflow
      ? 'bg-purple-500/10'
      : 'bg-emerald-500/10';

  return (
    <GlassNode bg={bgClass} className="min-w-[200px]">
      {/* Input Handle */}
      <Handle
        type="target"
        position={Position.Left}
        className="!w-2.5 !h-2.5 !bg-primary !border-2 !border-background !-left-1.5 shadow-lg"
      />

      <div className="flex items-start justify-between mb-4 relative z-10">
        <div
          className={cn(
            'p-2.5 rounded-xl transition-all duration-500 group-hover:scale-110 group-hover:rotate-3 shadow-inner ring-1 ring-white/5',
            bgClass,
          )}
        >
          <Icon className={cn('h-4 w-4', colorClass)} />
        </div>
        <div className="flex gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7 hover:bg-muted/50 text-foreground/70 hover:text-foreground"
            onClick={(e) => {
              e.stopPropagation();
              onEdit?.(id);
            }}
          >
            <Settings2 className="h-3.5 w-3.5" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7 hover:bg-red-500/20 text-red-500/50 hover:text-red-500"
            onClick={(e) => {
              e.stopPropagation();
              onRemove?.(id);
            }}
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>

      <div className="space-y-1 relative z-10">
        <h4 className="text-[14px] font-bold text-foreground truncate tracking-tight uppercase">
          {step.type === 'inline' ? step.processor : step.name}
        </h4>
        <div className="flex items-center justify-between">
          <span className="text-[9px] font-mono text-foreground/20 group-hover:text-foreground/40 transition-colors">
            #{id.slice(0, 8)}
          </span>
          <span
            className={cn(
              'text-[8px] font-black uppercase tracking-[0.15em]',
              colorClass,
            )}
          >
            {step.type}
          </span>
        </div>
      </div>

      {/* Output Handle */}
      <Handle
        type="source"
        position={Position.Right}
        className="!w-2.5 !h-2.5 !bg-primary !border-2 !border-background !-right-1.5 shadow-lg"
      />
    </GlassNode>
  );
}
