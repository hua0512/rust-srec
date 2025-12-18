import { useCallback, useMemo, useEffect } from 'react';
import {
    ReactFlow,
    Background,
    Controls,
    Connection,
    Edge,
    useNodesState,
    useEdgesState,
    MarkerType,
    Panel,
    ConnectionMode
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';

import { DagStepDefinition } from '@/api/schemas';
import { StepNode } from './step-node';
import { Button } from '@/components/ui/button';
import { LayoutGrid, MousePointer2 } from 'lucide-react';
import { getLayoutedElements } from './layout';

const nodeTypes = {
    stepNode: StepNode,
};

const defaultEdgeOptions = {
    style: { strokeWidth: 2, stroke: 'rgba(59, 130, 246, 0.5)' },
    type: 'smoothstep',
    markerEnd: {
        type: MarkerType.ArrowClosed,
        color: 'rgba(59, 130, 246, 0.5)',
    },
};

interface WorkflowFlowEditorProps {
    steps: DagStepDefinition[];
    onUpdateSteps: (steps: DagStepDefinition[]) => void;
    onEditStep?: (id: string) => void;
}

export function WorkflowFlowEditor({ steps, onUpdateSteps, onEditStep }: WorkflowFlowEditorProps) {
    // Generate initial nodes and edges
    const initialData = useMemo(() => {
        const nodes = steps.map((s, idx) => ({
            id: s.id,
            type: 'stepNode',
            position: { x: idx * 250, y: 100 }, // Initial casual layout
            data: {
                step: s.step,
                id: s.id,
                onEdit: onEditStep,
                onRemove: (id: string) => {
                    onUpdateSteps(steps.filter(st => st.id !== id));
                }
            },
        }));

        const edges = steps.flatMap(s =>
            (s.depends_on || []).map(depId => ({
                id: `e-${depId}-${s.id}`,
                source: depId,
                target: s.id,
            }))
        );

        const layouted = getLayoutedElements(nodes, edges);
        return { nodes: layouted.nodes, edges: layouted.edges };
    }, [steps, onEditStep, onUpdateSteps]);

    const [nodes, setNodes, onNodesChange] = useNodesState(initialData.nodes);
    const [edges, setEdges, onEdgesChange] = useEdgesState(initialData.edges);

    // Sync from props
    useEffect(() => {
        setNodes(initialData.nodes);
        setEdges(initialData.edges);
    }, [initialData.nodes, initialData.edges, setNodes, setEdges]);

    const onConnect = useCallback(
        (params: Connection) => {
            if (!params.source || !params.target) return;

            // Avoid duplicate edges
            if (edges.some(e => e.source === params.source && e.target === params.target)) return;

            // Update parent state
            const updatedSteps = steps.map(s => {
                if (s.id === params.target) {
                    return {
                        ...s,
                        depends_on: [...new Set([...(s.depends_on || []), params.source!])]
                    };
                }
                return s;
            });
            onUpdateSteps(updatedSteps);
        },
        [edges, steps, onUpdateSteps]
    );

    const onEdgeDelete = useCallback((edgesToDelete: Edge[]) => {
        const updatedSteps = steps.map(s => ({
            ...s,
            depends_on: (s.depends_on || []).filter(
                depId => !edgesToDelete.some(e => e.source === depId && e.target === s.id)
            )
        }));
        onUpdateSteps(updatedSteps);
    }, [steps, onUpdateSteps]);

    const handleLayout = useCallback(() => {
        const { nodes: layoutedNodes, edges: layoutedEdges } = getLayoutedElements(
            nodes,
            edges
        );
        setNodes([...layoutedNodes]);
        setEdges([...layoutedEdges]);
    }, [nodes, edges, setNodes, setEdges]);

    return (
        <div className="w-full h-full min-h-[600px] bg-background/50 backdrop-blur-sm rounded-2xl border border-border/40 relative overflow-hidden">
            <ReactFlow
                nodes={nodes}
                edges={edges}
                onNodesChange={onNodesChange}
                onEdgesChange={onEdgesChange}
                onConnect={onConnect}
                onEdgesDelete={onEdgeDelete}
                nodeTypes={nodeTypes}
                defaultEdgeOptions={defaultEdgeOptions}
                connectionMode={ConnectionMode.Loose}
                fitView
                colorMode="system"
            >
                <Background color="currentColor" gap={20} size={1} className="opacity-[0.05]" />
                <Controls className="!bg-background/80 !border-border/40 !backdrop-blur-md" />

                <Panel position="top-right" className="flex gap-2">
                    <div className="bg-background/80 backdrop-blur-md border border-border/40 rounded-lg p-1 flex gap-1 shadow-sm">
                        <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            className="h-8 px-3 text-muted-foreground hover:text-foreground"
                            onClick={handleLayout}
                        >
                            <LayoutGrid className="h-3.5 w-3.5 mr-2" />
                            <span className="text-[10px] font-bold uppercase tracking-wider">Auto Layout</span>
                        </Button>
                    </div>
                </Panel>

                <Panel position="bottom-center">
                    <div className="bg-primary/5 backdrop-blur-md border border-primary/20 rounded-full px-4 py-1.5 flex items-center gap-3 shadow-lg">
                        <MousePointer2 className="h-3.5 w-3.5 text-blue-500" />
                        <span className="text-[10px] font-bold uppercase tracking-widest text-blue-400">
                            Drag from edge to link steps
                        </span>
                    </div>
                </Panel>
            </ReactFlow>
        </div>
    );
}
