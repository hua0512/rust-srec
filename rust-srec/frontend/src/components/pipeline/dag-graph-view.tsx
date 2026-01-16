import { useMemo, useState } from 'react';
import { Link } from '@tanstack/react-router';
import { motion, useMotionValue, useSpring } from 'framer-motion';
import { DagGraph, DagGraphNode, DagStepStatus } from '@/api/schemas';
import { cn } from '@/lib/utils';
import {
  CheckCircle2,
  Clock,
  RefreshCw,
  XCircle,
  AlertCircle,
  Maximize2,
  Minimize2,
  Move,
} from 'lucide-react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { GraphViewport, GlassNode } from './graph-shared';
import { useLingui } from '@lingui/react';
import { t } from '@lingui/core/macro';
import { Trans } from '@lingui/react/macro';
import { getJobPresetName } from './presets/default-presets-i18n';
import { getProcessorDefinition } from './presets/processors/registry';

interface DagGraphViewProps {
  graph: DagGraph;
  className?: string;
}

const STATUS_CONFIG: Record<
  DagStepStatus,
  {
    icon: React.ElementType;
    color: string;
    bg: string;
    glow: string;
    border: string;
    animate?: boolean;
  }
> = {
  BLOCKED: {
    icon: Clock,
    color: 'text-muted-foreground/40',
    bg: 'bg-muted/5',
    glow: 'shadow-transparent',
    border: 'border-white/5',
  },
  PENDING: {
    icon: Clock,
    color: 'text-muted-foreground',
    bg: 'bg-muted/10',
    glow: 'shadow-transparent',
    border: 'border-white/10',
  },
  PROCESSING: {
    icon: RefreshCw,
    color: 'text-blue-500',
    bg: 'bg-blue-500/10',
    glow: 'shadow-blue-500/20',
    border: 'border-blue-500/30',
    animate: true,
  },
  COMPLETED: {
    icon: CheckCircle2,
    color: 'text-emerald-500',
    bg: 'bg-emerald-500/10',
    glow: 'shadow-emerald-500/10',
    border: 'border-emerald-500/20',
  },
  FAILED: {
    icon: XCircle,
    color: 'text-red-500',
    bg: 'bg-red-500/10',
    glow: 'shadow-red-500/20',
    border: 'border-red-500/20',
  },
  CANCELLED: {
    icon: AlertCircle,
    color: 'text-gray-500',
    bg: 'bg-gray-500/10',
    glow: 'shadow-transparent',
    border: 'border-gray-500/20',
  },
};

export function DagGraphView({ graph, className }: DagGraphViewProps) {
  const { i18n } = useLingui();
  const [scale, setScale] = useState(1);
  const x = useMotionValue(0);
  const y = useMotionValue(0);

  // Smooth springs for movement
  const springX = useSpring(x, { stiffness: 300, damping: 30 });
  const springY = useSpring(y, { stiffness: 300, damping: 30 });

  // Simple layering algorithm
  const layout = useMemo(() => {
    const { nodes, edges } = graph;
    const levels: Record<string, number> = {};
    const nodeMap: Record<string, DagGraphNode> = {};
    nodes.forEach((n) => (nodeMap[n.id] = n));

    const getLevel = (id: string, visited = new Set<string>()): number => {
      if (levels[id] !== undefined) return levels[id];
      if (visited.has(id)) return 0; // Cycle safety
      visited.add(id);

      const incoming = edges.filter((e) => e.to === id);
      if (incoming.length === 0) {
        levels[id] = 0;
        return 0;
      }

      const maxLevel = Math.max(
        ...incoming.map((e) => getLevel(e.from, visited)),
        -1,
      );
      levels[id] = maxLevel + 1;
      return levels[id];
    };

    nodes.forEach((n) => getLevel(n.id));

    const nodesByLevel: DagGraphNode[][] = [];
    Object.entries(levels).forEach(([id, level]) => {
      if (!nodesByLevel[level]) nodesByLevel[level] = [];
      nodesByLevel[level].push(nodeMap[id]);
    });

    return nodesByLevel;
  }, [graph]);

  const { nodePositions, totalWidth, totalHeight } = useMemo(() => {
    const positions: Record<string, { x: number; y: number }> = {};
    const LEVEL_SPACING = 350;
    const NODE_SPACING = 180;

    const maxNodesPerLevel = Math.max(...layout.map((l) => l.length), 1);
    const maxHeight = maxNodesPerLevel * NODE_SPACING;

    layout.forEach((nodes, levelIdx) => {
      const levelHeight = (nodes.length - 1) * NODE_SPACING;
      const offset = (maxHeight - levelHeight) / 2;
      nodes.forEach((node, nodeIdx) => {
        positions[node.id] = {
          x: levelIdx * LEVEL_SPACING + 100,
          y: offset + nodeIdx * NODE_SPACING + 100,
        };
      });
    });

    return {
      nodePositions: positions,
      totalWidth: layout.length * LEVEL_SPACING + 300,
      totalHeight: maxHeight + 200,
    };
  }, [layout]);

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
          >
            <Maximize2 className="h-4 w-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 hover:bg-muted/50 text-foreground/70"
            onClick={zoomOut}
          >
            <Minimize2 className="h-4 w-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 hover:bg-muted/50 text-foreground/70"
            onClick={handleReset}
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
          width: totalWidth,
          height: totalHeight,
        }}
        className="relative active:cursor-grabbing"
      >
        {/* SVG Layer */}
        <svg className="absolute inset-0 w-full h-full pointer-events-none overflow-visible">
          <defs>
            <linearGradient id="edgeGradient" x1="0%" y1="0%" x2="100%" y2="0%">
              <stop offset="0%" stopColor="rgba(59, 130, 246, 0.2)" />
              <stop offset="50%" stopColor="rgba(59, 130, 246, 0.5)" />
              <stop offset="100%" stopColor="rgba(59, 130, 246, 0.2)" />
            </linearGradient>
            <marker
              id="premium-arrowhead"
              markerWidth="10"
              markerHeight="10"
              refX="9"
              refY="5"
              orient="auto"
            >
              <path d="M0,0 L10,5 L0,10 Z" fill="rgb(59, 130, 246)" />
            </marker>
          </defs>

          {graph.edges.map((edge, idx) => {
            const from = nodePositions[edge.from];
            const to = nodePositions[edge.to];
            if (!from || !to) return null;

            const fromNode = graph.nodes.find((n) => n.id === edge.from);
            const isActive =
              fromNode?.status === 'PROCESSING' ||
              fromNode?.status === 'COMPLETED';

            // Calculate points - arrows connect at node edges only
            const startX = from.x + 180; // Right edge of source node
            const startY = from.y + 45; // Vertical center
            const endX = to.x; // Left edge of target node
            const endY = to.y + 45; // Vertical center
            const midX = (startX + endX) / 2;
            const pathString = `M ${startX} ${startY} C ${midX} ${startY}, ${midX} ${endY}, ${endX} ${endY}`;

            // Shorter path for flow animation (doesn't need marker)
            const flowEndX = to.x - 15;
            const flowMidX = (startX + flowEndX) / 2;
            const flowPathString = `M ${startX} ${startY} C ${flowMidX} ${startY}, ${flowMidX} ${endY}, ${flowEndX} ${endY}`;

            return (
              <g key={`${edge.from}-${edge.to}-${idx}`}>
                {/* Shadow/Glow Path */}
                {isActive && (
                  <path
                    d={flowPathString}
                    fill="none"
                    stroke="rgba(59, 130, 246, 0.08)"
                    strokeWidth="6"
                    className="blur-[6px]"
                  />
                )}
                {/* Base Path */}
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

                {/* Flow Animation */}
                {isActive && (
                  <motion.path
                    d={flowPathString}
                    fill="none"
                    stroke="url(#edgeGradient)"
                    strokeWidth="3.5"
                    strokeDasharray="12, 24"
                    animate={{
                      strokeDashoffset: [0, -72],
                    }}
                    transition={{
                      duration: 1.5,
                      repeat: Infinity,
                      ease: 'linear',
                    }}
                  />
                )}
              </g>
            );
          })}
        </svg>

        {/* Nodes Layer */}
        <div className="relative z-10">
          {graph.nodes.map((node, idx) => {
            const pos = nodePositions[node.id];
            const config = STATUS_CONFIG[node.status];
            const Icon = config.icon;

            const nodeContent = (
              <GlassNode
                glow={config.glow}
                bg={config.bg}
                isClickable={!!node.job_id}
                className="w-full"
              >
                {node.job_id && (
                  <div className="absolute -top-2 -right-2 opacity-0 group-hover:opacity-100 transition-opacity z-20">
                    <div className="bg-primary text-primary-foreground text-[8px] font-black uppercase px-2 py-0.5 rounded-full shadow-lg ring-1 ring-white/20">
                      <Trans>View</Trans>
                    </div>
                  </div>
                )}

                <div className="flex items-start justify-between mb-4 relative z-10">
                  <div
                    className={cn(
                      'p-2.5 rounded-xl transition-all duration-500 group-hover:scale-110 group-hover:rotate-3 shadow-inner ring-1 ring-white/5',
                      config.bg,
                    )}
                  >
                    <Icon
                      className={cn(
                        'h-4 w-4',
                        config.color,
                        config.animate && 'animate-spin',
                      )}
                    />
                  </div>
                  <Badge
                    variant="outline"
                    className="bg-foreground/5 border-foreground/10 text-[9px] uppercase tracking-widest font-black opacity-40 group-hover:opacity-80 transition-opacity"
                  >
                    {(() => {
                      const def = getProcessorDefinition(node.processor || '');
                      return def ? i18n._(def.label) : node.processor || '';
                    })()}
                  </Badge>
                </div>

                <h4 className="text-[15px] font-bold truncate text-foreground mb-1 tracking-tight relative z-10 uppercase">
                  {getJobPresetName(
                    { id: node.label || node.id, name: node.label || node.id },
                    i18n,
                  )}
                </h4>

                <div className="flex items-center justify-between relative z-10">
                  <span
                    className={cn(
                      'text-[10px] font-black uppercase tracking-widest',
                      config.color,
                    )}
                  >
                    {i18n._(
                      node.status === 'BLOCKED'
                        ? t`Blocked`
                        : node.status === 'PENDING'
                          ? t`Pending`
                          : node.status === 'PROCESSING'
                            ? t`Processing`
                            : node.status === 'COMPLETED'
                              ? t`Completed`
                              : node.status === 'FAILED'
                                ? t`Failed`
                                : node.status === 'CANCELLED'
                                  ? t`Cancelled`
                                  : node.status,
                    )}
                  </span>
                  {node.job_id && (
                    <span className="text-[9px] font-mono text-foreground/20 group-hover:text-foreground/40 transition-colors">
                      #{node.job_id.slice(0, 8)}
                    </span>
                  )}
                </div>

                {/* Animated progress bar for processing */}
                {node.status === 'PROCESSING' && (
                  <div className="absolute bottom-0 left-0 right-0 h-1 overflow-hidden rounded-b-2xl">
                    <motion.div
                      className="h-full bg-blue-500 shadow-[0_0_10px_rgba(59,130,246,0.8)]"
                      animate={{
                        x: ['-100%', '100%'],
                      }}
                      transition={{
                        duration: 2,
                        repeat: Infinity,
                        ease: 'linear',
                      }}
                    />
                  </div>
                )}
              </GlassNode>
            );

            return (
              <motion.div
                key={node.id}
                initial={{ opacity: 0, y: 20 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ delay: idx * 0.05 + 0.3, type: 'spring' }}
                style={{
                  left: pos.x,
                  top: pos.y,
                  width: 180,
                }}
                className="absolute"
              >
                {node.job_id ? (
                  <Link
                    to="/pipeline/jobs/$jobId"
                    params={{ jobId: node.job_id }}
                  >
                    {nodeContent}
                  </Link>
                ) : (
                  nodeContent
                )}
              </motion.div>
            );
          })}
        </div>
      </motion.div>
    </GraphViewport>
  );
}
