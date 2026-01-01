import { useCallback, useEffect, useState, memo } from 'react';
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
  ConnectionMode,
  Node,
  useReactFlow,
  ReactFlowProvider,
  useNodesInitialized,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';

import { GraphViewport } from '../../graph-shared';
import { DagStepDefinition } from '@/api/schemas';
import { StepNode } from './step-node';
import { Button } from '@/components/ui/button';
import { LayoutGrid } from 'lucide-react';
import { getLayoutedElements } from './layout';
import { Trans } from '@lingui/react/macro';

const nodeTypes = {
  stepNode: StepNode,
};

const defaultEdgeOptions = {
  style: { strokeWidth: 1.5, stroke: 'rgba(59, 130, 246, 0.4)' },
  type: 'smoothstep',
  markerEnd: {
    type: MarkerType.ArrowClosed,
    color: 'rgba(59, 130, 246, 0.4)',
  },
};

interface WorkflowFlowEditorProps {
  steps: DagStepDefinition[];
  onUpdateSteps: (steps: DagStepDefinition[]) => void;
  onEditStep?: (id: string) => void;
  onRemoveStep?: (id: string) => void;
}

const WorkflowFlowEditorInner = memo(
  ({
    steps,
    onUpdateSteps,
    onEditStep,
    onRemoveStep,
  }: WorkflowFlowEditorProps) => {
    const [nodes, setNodes, onNodesChange] = useNodesState<Node>([]);
    const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
    const [hasLaidOut, setHasLaidOut] = useState(false);
    const nodesInitialized = useNodesInitialized();
    const { fitView } = useReactFlow();

    // Memoized remove handler to avoid creating new functions on every render
    const handleRemoveStep = useCallback(
      (id: string) => {
        if (onRemoveStep) {
          onRemoveStep(id);
        } else {
          onUpdateSteps(steps.filter((st) => st.id !== id));
        }
      },
      [steps, onUpdateSteps, onRemoveStep],
    );

    // Sync from props
    useEffect(() => {
      // 1. Generate Edges from steps (Always fresh)
      const newEdges = steps.flatMap((s) =>
        (s.depends_on || []).map((depId) => ({
          id: `e-${depId}-${s.id}`,
          source: depId,
          target: s.id,
          ...defaultEdgeOptions,
        })),
      );

      // 2. Generate Nodes (Merge strategy)
      // We use a functional update to ensure we're working with the very latest node state
      // when calculating the merge, preventing stale closures.
      setNodes((currentNodes) => {
        const currentNodeMap = new Map(currentNodes.map((n) => [n.id, n]));

        const nextNodes = steps.map((s) => {
          const existing = currentNodeMap.get(s.id);

          // Common data payload
          const nodeData = {
            step: s.step,
            id: s.id,
            onEdit: onEditStep,
            onRemove: handleRemoveStep,
          };

          if (existing) {
            // Update existing node (preserve position/dimensions)
            return {
              ...existing,
              data: nodeData,
            };
          }

          // New node
          return {
            id: s.id,
            type: 'stepNode',
            position: { x: 0, y: 0 }, // Will be fixed by layout if it's initial load
            data: nodeData,
          };
        });

        // If we haven't performed an initial layout yet and we have nodes, do it now.
        // Note: We check `currentNodes.length === 0` as a proxy for "first load"
        // combined with the external `hasLaidOut` ref check would be ideal,
        // but here we can't easily access the updated 'hasLaidOut' state inside this callback
        // if we wanted to set it.
        // Instead, we'll handle the "Auto Layout" transformation here and rely on the effect to stabilize.

        return nextNodes;
      });

      setEdges(newEdges);
    }, [steps, onEditStep, handleRemoveStep, setNodes, setEdges]);

    // Separate effect to handle Auto-Layout logic
    // We want to re-layout when the structure changes (e.g. loading a preset)
    // We detect this by checking if we have unlayouted nodes (position 0,0) or if the step count changed significantly
    useEffect(() => {
      // Only run layout if:
      // 1. We have nodes
      // 2. Nodes are initialized (measured by React Flow)
      // 3. We haven't laid out yet OR nodes are all at 0,0 (new load)
      const needsLayout =
        nodes.length > 0 &&
        nodesInitialized &&
        (!hasLaidOut ||
          nodes.every((n) => n.position.x === 0 && n.position.y === 0));

      if (needsLayout) {
        console.log('WorkflowFlowEditor: Running auto-layout', {
          nodeCount: nodes.length,
        });
        const { nodes: layoutedNodes, edges: layoutedEdges } =
          getLayoutedElements(nodes, edges);

        setNodes([...layoutedNodes]);
        setEdges([...layoutedEdges]);
        setHasLaidOut(true);

        window.requestAnimationFrame(() => {
          fitView({ padding: 0.2, duration: 200 });
        });
      }
    }, [
      nodes,
      edges,
      nodesInitialized,
      hasLaidOut,
      setNodes,
      setEdges,
      fitView,
      nodes.length,
    ]);

    const onConnect = useCallback(
      (params: Connection) => {
        if (!params.source || !params.target) return;

        // Avoid duplicate edges
        if (
          edges.some(
            (e) => e.source === params.source && e.target === params.target,
          )
        )
          return;

        // Update parent state
        const updatedSteps = steps.map((s) => {
          if (s.id === params.target) {
            return {
              ...s,
              depends_on: [
                ...new Set([...(s.depends_on || []), params.source!]),
              ],
            };
          }
          return s;
        });
        onUpdateSteps(updatedSteps);
      },
      [edges, steps, onUpdateSteps],
    );

    const onEdgeDelete = useCallback(
      (edgesToDelete: Edge[]) => {
        const updatedSteps = steps.map((s) => ({
          ...s,
          depends_on: (s.depends_on || []).filter(
            (depId) =>
              !edgesToDelete.some(
                (e) => e.source === depId && e.target === s.id,
              ),
          ),
        }));
        onUpdateSteps(updatedSteps);
      },
      [steps, onUpdateSteps],
    );

    const handleLayout = useCallback(() => {
      const { nodes: layoutedNodes, edges: layoutedEdges } =
        getLayoutedElements(nodes, edges);
      setNodes([...layoutedNodes]);
      setEdges([...layoutedEdges]);
    }, [nodes, edges, setNodes, setEdges]);

    return (
      <GraphViewport className="h-full min-h-[600px]">
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
          fitView={!hasLaidOut} // Only fit view on initial load
          colorMode="system"
        >
          <Background
            variant={'dots' as any}
            color="currentColor"
            gap={20}
            size={1}
            className="opacity-[0.02]"
          />
          <Controls className="!bg-background/40 !border-border/40 !backdrop-blur-md !shadow-2xl !rounded-lg overflow-hidden" />

          <Panel position="top-right" className="flex gap-2">
            <div className="bg-background/40 backdrop-blur-md border border-border/40 rounded-lg p-1 flex gap-1 shadow-2xl">
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="h-8 px-3 hover:bg-muted/50 text-foreground/70"
                onClick={handleLayout}
              >
                <LayoutGrid className="h-3.5 w-3.5 mr-2" />
                <span className="text-[10px] font-bold uppercase tracking-wider">
                  <Trans>Auto Layout</Trans>
                </span>
              </Button>
            </div>
          </Panel>

          <Panel position="bottom-center">
            <div className="bg-card/70 backdrop-blur-md border border-primary/20 rounded-full px-5 py-2 flex items-center gap-3 shadow-2xl">
              <div className="flex relative h-2 w-2">
                <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-blue-400 opacity-75"></span>
                <span className="relative inline-flex rounded-full h-2 w-2 bg-blue-500"></span>
              </div>
              <span className="text-[10px] font-black uppercase tracking-[0.2em] text-foreground/60">
                <Trans>Drag from edge to link steps</Trans>
              </span>
            </div>
          </Panel>
        </ReactFlow>
      </GraphViewport>
    );
  },
);

WorkflowFlowEditorInner.displayName = 'WorkflowFlowEditorInner';

export const WorkflowFlowEditor = memo((props: WorkflowFlowEditorProps) => {
  return (
    <ReactFlowProvider>
      <WorkflowFlowEditorInner {...props} />
    </ReactFlowProvider>
  );
});

WorkflowFlowEditor.displayName = 'WorkflowFlowEditor';
