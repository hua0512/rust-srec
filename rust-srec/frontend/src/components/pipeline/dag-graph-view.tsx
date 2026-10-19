import { memo, useMemo, useState } from 'react';
import { Link } from '@tanstack/react-router';
import { motion, useMotionValue, useSpring } from 'motion/react';
import type { DagStep } from '@/api/schemas';
import { cn } from '@/lib/utils';
import { Maximize2, Minimize2, Move } from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Progress } from '@/components/ui/progress';
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from '@/components/ui/tooltip';
import { GraphViewport, GlassNode } from './graph-shared';
import { computeNodeLevels } from './graph-levels';
import { useLingui } from '@lingui/react';
import { msg } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { getJobPresetName } from './presets/default-presets-i18n';
import { getProcessorDefinition } from './presets/processors/registry';
import { StepActivity, stepProgress } from './step-activity';
import {
  getStatusConfig,
  getStatusLabel,
  stepDisplayStatus,
} from '@/components/pipeline/status-config';

interface DagGraphViewProps {
  steps: DagStep[];
  className?: string;
}

type NodePosition = { x: number; y: number };

// Each step's id with the ids of the steps it depends on.
type Topology = [id: string, dependsOn: string[]][];

const LEVEL_SPACING = 350;
const NODE_SPACING = 180;

/**
 * Places each step in the column of its depth from a root and lists the edges
 * between known steps, from a dependency to its dependent.
 */
function layoutGraph(topology: Topology) {
  const ids = topology.map(([id]) => id);
  const known = new Set(ids);
  const edges = topology.flatMap(([id, dependsOn]) =>
    dependsOn
      .filter((dependency) => known.has(dependency))
      .map((dependency) => ({ from: dependency, to: id })),
  );
  const levels = computeNodeLevels(
    ids,
    edges.map((edge) => ({ source: edge.from, target: edge.to })),
  );

  const idsByLevel: string[][] = [];
  Object.entries(levels).forEach(([id, level]) => {
    if (!idsByLevel[level]) idsByLevel[level] = [];
    idsByLevel[level].push(id);
  });

  const maxNodesPerLevel = Math.max(...idsByLevel.map((l) => l.length), 1);
  const maxHeight = maxNodesPerLevel * NODE_SPACING;
  const positions: Record<string, NodePosition> = {};
  idsByLevel.forEach((levelIds, levelIdx) => {
    const levelHeight = (levelIds.length - 1) * NODE_SPACING;
    const offset = (maxHeight - levelHeight) / 2;
    levelIds.forEach((id, nodeIdx) => {
      positions[id] = {
        x: levelIdx * LEVEL_SPACING + 100,
        y: offset + nodeIdx * NODE_SPACING + 100,
      };
    });
  });

  return {
    positions,
    edges,
    width: idsByLevel.length * LEVEL_SPACING + 300,
    height: maxHeight + 200,
  };
}

export function DagGraphView({ steps, className }: DagGraphViewProps) {
  const { i18n } = useLingui();
  const [scale, setScale] = useState(1);
  const x = useMotionValue(0);
  const y = useMotionValue(0);

  // Smooth springs for movement
  const springX = useSpring(x, { stiffness: 300, damping: 30 });
  const springY = useSpring(y, { stiffness: 300, damping: 30 });

  // Keyed on the step ids and dependencies alone, so the status polls keep
  // the same positions and leave unchanged nodes alone.
  const topology = JSON.stringify(
    steps.map((step) => [step.step_id, step.depends_on]),
  );
  const layout = useMemo(
    () => layoutGraph(JSON.parse(topology) as Topology),
    [topology],
  );
  const stepById = useMemo(
    () => new Map(steps.map((step) => [step.step_id, step])),
    [steps],
  );

  const handleReset = () => {
    x.set(0);
    y.set(0);
    setScale(1);
  };

  const zoomIn = () => setScale((s) => Math.min(s + 0.1, 2));
  const zoomOut = () => setScale((s) => Math.max(s - 0.1, 0.5));

  return (
    <GraphViewport className={cn('h-[600px]', className)}>
      {/* Controls */}
      <div className="absolute top-4 right-4 z-50 flex flex-col gap-2 opacity-0 group-hover/graph:opacity-100 transition-opacity duration-300">
        <div className="bg-background/40 backdrop-blur-md rounded-lg border border-border/40 p-1 flex flex-col gap-1 shadow-2xl">
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 hover:bg-muted/50 text-foreground/70"
            onClick={zoomIn}
            aria-label={i18n._(msg`Zoom in`)}
          >
            <Maximize2 className="h-4 w-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 hover:bg-muted/50 text-foreground/70"
            onClick={zoomOut}
            aria-label={i18n._(msg`Zoom out`)}
          >
            <Minimize2 className="h-4 w-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 hover:bg-muted/50 text-foreground/70"
            onClick={handleReset}
            aria-label={i18n._(msg`Reset view`)}
          >
            <Move className="h-4 w-4" />
          </Button>
        </div>
      </div>

      <motion.div
        drag
        dragMomentum={false}
        style={{
          x: springX,
          y: springY,
          scale,
          cursor: 'grab',
          width: layout.width,
          height: layout.height,
        }}
        className="relative active:cursor-grabbing"
      >
        {/* SVG Layer */}
        <svg className="absolute inset-0 w-full h-full pointer-events-none overflow-visible">
          {layout.edges.map((edge, idx) => {
            const from = layout.positions[edge.from];
            const to = layout.positions[edge.to];
            const fromStatus = stepById.get(edge.from)?.status;
            const isActive =
              fromStatus === 'PROCESSING' || fromStatus === 'COMPLETED';

            // Calculate points - arrows connect at node edges only
            const startX = from.x + 180; // Right edge of source node
            const startY = from.y + 45; // Vertical center
            const endX = to.x; // Left edge of target node
            const endY = to.y + 45; // Vertical center
            const midX = (startX + endX) / 2;
            const pathString = `M ${startX} ${startY} C ${midX} ${startY}, ${midX} ${endY}, ${endX} ${endY}`;

            return (
              <g key={`${edge.from}-${edge.to}-${idx}`}>
                <motion.path
                  d={pathString}
                  fill="none"
                  stroke={
                    isActive
                      ? 'rgba(59, 130, 246, 0.3)'
                      : 'rgba(255, 255, 255, 0.04)'
                  }
                  strokeWidth="2"
                  initial={{ pathLength: 0 }}
                  animate={{ pathLength: 1 }}
                  transition={{
                    duration: 1.2,
                    ease: 'easeInOut',
                    delay: idx * 0.05,
                  }}
                />

                {/* Arrowhead - appears after line draws */}
                <motion.polygon
                  points={`${endX - 14},${endY - 7} ${endX},${endY} ${endX - 14},${endY + 7}`}
                  fill="rgb(59, 130, 246)"
                  initial={{ opacity: 0, scale: 0.5 }}
                  animate={{ opacity: 1, scale: 1 }}
                  transition={{
                    duration: 0.3,
                    delay: idx * 0.05 + 1.0,
                    ease: 'easeOut',
                  }}
                  style={{ transformOrigin: `${endX}px ${endY}px` }}
                />
              </g>
            );
          })}
        </svg>

        {/* Nodes Layer */}
        <div className="relative z-10">
          {steps.map((step, idx) => (
            <DagNode
              key={step.step_id}
              step={step}
              position={layout.positions[step.step_id]}
              index={idx}
            />
          ))}
        </div>
      </motion.div>
    </GraphViewport>
  );
}

const DagNode = memo(function DagNode({
  step,
  position,
  index,
}: {
  step: DagStep;
  position: NodePosition;
  index: number;
}) {
  const { i18n } = useLingui();
  const status = stepDisplayStatus(step);
  const config = getStatusConfig(status);
  const Icon = config.icon;
  const { percent } = stepProgress(step, i18n);
  const label = step.processor ?? step.step_id;
  const definition = getProcessorDefinition(step.processor ?? '');

  const nodeContent = (
    <GlassNode
      glow={config.glow}
      bg={config.surfaceBg}
      isClickable={!!step.job_id}
      className="w-full"
      overlay={
        percent != null && (
          <Progress
            value={percent}
            className="absolute inset-x-0 bottom-0 h-1 rounded-none bg-blue-500/10"
            indicatorClassName="bg-blue-500"
          />
        )
      }
    >
      {step.job_id && (
        <div className="absolute -top-2 -right-2 opacity-0 group-hover:opacity-100 transition-opacity z-20">
          <div className="bg-primary text-primary-foreground text-[8px] font-black uppercase px-2 py-0.5 rounded-full shadow-lg ring-1 ring-white/20">
            <Trans>View</Trans>
          </div>
        </div>
      )}

      <div className="flex items-start justify-between gap-2 mb-4 relative z-10">
        <div
          className={cn(
            'p-2.5 rounded-xl transition-all duration-500 group-hover:scale-110 group-hover:rotate-3 shadow-inner ring-1 ring-white/5',
            config.surfaceBg,
          )}
        >
          <Icon className={cn('h-4 w-4', config.textColor)} />
        </div>
        <Badge
          variant="outline"
          className="min-w-0 shrink bg-foreground/5 border-foreground/10 text-[9px] uppercase tracking-widest font-black opacity-40 group-hover:opacity-80 transition-opacity"
        >
          <span className="truncate">
            {definition ? i18n._(definition.label) : step.processor}
          </span>
        </Badge>
      </div>

      <h4 className="text-[15px] font-bold truncate text-foreground mb-1 tracking-tight relative z-10 uppercase">
        {getJobPresetName({ id: label, name: label }, i18n)}
      </h4>

      <div className="flex items-center justify-between relative z-10">
        <span
          className={cn(
            'flex items-baseline gap-1.5 text-[10px] font-black uppercase tracking-widest',
            config.textColor,
          )}
        >
          <span>{getStatusLabel(i18n, status)}</span>
          {percent != null && (
            <span className="tabular-nums">{`${percent.toFixed(0)}%`}</span>
          )}
        </span>
        {step.job_id && (
          <span className="text-[9px] font-mono text-foreground/20 group-hover:text-foreground/40 transition-colors">
            #{step.job_id.slice(0, 8)}
          </span>
        )}
      </div>
    </GlassNode>
  );

  const node = step.job_id ? (
    <Link to="/pipeline/jobs/$jobId" params={{ jobId: step.job_id }}>
      {nodeContent}
    </Link>
  ) : (
    nodeContent
  );

  return (
    <motion.div
      initial={{ opacity: 0, y: 20 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: index * 0.05 + 0.3, type: 'spring' }}
      style={{
        left: position.x,
        top: position.y,
        width: 180,
      }}
      className="absolute"
    >
      {status === 'RUNNING' || status === 'RETRYING' ? (
        <Tooltip>
          <TooltipTrigger asChild>
            <div>{node}</div>
          </TooltipTrigger>
          <TooltipContent side="bottom">
            <StepActivity step={step} />
          </TooltipContent>
        </Tooltip>
      ) : (
        node
      )}
    </motion.div>
  );
});
