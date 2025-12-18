import { Handle, Position, NodeProps, type Node } from '@xyflow/react';
import { cn } from '@/lib/utils';
import { DagStepDefinition } from '@/api/schemas';
import {
    Settings2,
    Trash2,
    Database,
    Zap,
    Box
} from 'lucide-react';
import { Button } from '@/components/ui/button';

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
    const colorClass = isPreset ? 'text-blue-500' : isWorkflow ? 'text-purple-500' : 'text-emerald-500';
    const bgClass = isPreset ? 'bg-blue-500/10' : isWorkflow ? 'bg-purple-500/10' : 'bg-emerald-500/10';

    return (
        <div className="group relative min-w-[200px] p-4 rounded-2xl border border-border bg-card shadow-sm transition-all hover:border-primary/50 hover:shadow-md hover:scale-[1.02]">
            {/* Input Handle */}
            <Handle
                type="target"
                position={Position.Left}
                className="!w-3 !h-3 !bg-blue-500 !border-2 !border-background"
            />

            <div className="flex items-start justify-between mb-3">
                <div className={cn("p-2 rounded-xl", bgClass)}>
                    <Icon className={cn("h-4 w-4", colorClass)} />
                </div>
                <div className="flex gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                    <Button
                        type="button"
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7 hover:bg-muted text-muted-foreground hover:text-foreground"
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

            <div className="space-y-1">
                <h4 className="text-sm font-bold text-foreground truncate tracking-tight">
                    {step.type === 'inline' ? step.processor : step.name}
                </h4>
                <div className="flex items-center gap-2">
                    <span className="text-[10px] font-mono text-muted-foreground truncate">
                        {id}
                    </span>
                    <span className={cn("text-[9px] font-bold uppercase tracking-widest", colorClass)}>
                        {step.type}
                    </span>
                </div>
            </div>

            {/* Output Handle */}
            <Handle
                type="source"
                position={Position.Right}
                className="!w-3 !h-3 !bg-blue-500 !border-2 !border-background"
            />
        </div>
    );
}
