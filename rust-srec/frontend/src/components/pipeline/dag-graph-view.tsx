import { useMemo, useState, type ReactNode } from 'react';
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
    Move
} from 'lucide-react';
import { Button } from '@/components/ui/button';

interface DagGraphViewProps {
    graph: DagGraph;
    className?: string;
}

const STATUS_CONFIG: Record<DagStepStatus, {
    icon: any,
    color: string,
    bg: string,
    shadow: string,
    animate?: boolean
}> = {
    BLOCKED: {
        icon: Clock,
        color: 'text-muted-foreground',
        bg: 'bg-muted/10',
        shadow: 'shadow-muted/5',
    },
    PENDING: {
        icon: Clock,
        color: 'text-orange-500',
        bg: 'bg-orange-500/10',
        shadow: 'shadow-orange-500/10',
    },
    PROCESSING: {
        icon: RefreshCw,
        color: 'text-blue-500',
        bg: 'bg-blue-500/20',
        shadow: 'shadow-blue-500/20',
        animate: true,
    },
    COMPLETED: {
        icon: CheckCircle2,
        color: 'text-emerald-500',
        bg: 'bg-emerald-500/10',
        shadow: 'shadow-emerald-500/10',
    },
    FAILED: {
        icon: XCircle,
        color: 'text-red-500',
        bg: 'bg-red-500/10',
        shadow: 'shadow-red-500/10',
    },
    CANCELLED: {
        icon: AlertCircle,
        color: 'text-gray-500',
        bg: 'bg-gray-500/10',
        shadow: 'shadow-gray-500/10',
    },
};

export function DagGraphView({ graph, className }: DagGraphViewProps) {
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
        nodes.forEach(n => nodeMap[n.id] = n);

        const getLevel = (id: string, visited = new Set<string>()): number => {
            if (levels[id] !== undefined) return levels[id];
            if (visited.has(id)) return 0; // Cycle safety
            visited.add(id);

            const incoming = edges.filter(e => e.to === id);
            if (incoming.length === 0) {
                levels[id] = 0;
                return 0;
            }

            const maxLevel = Math.max(...incoming.map(e => getLevel(e.from, visited)), -1);
            levels[id] = maxLevel + 1;
            return levels[id];
        };

        nodes.forEach(n => getLevel(n.id));

        const nodesByLevel: DagGraphNode[][] = [];
        Object.entries(levels).forEach(([id, level]) => {
            if (!nodesByLevel[level]) nodesByLevel[level] = [];
            nodesByLevel[level].push(nodeMap[id]);
        });

        return nodesByLevel;
    }, [graph]);

    const { nodePositions, totalWidth, totalHeight } = useMemo(() => {
        const positions: Record<string, { x: number, y: number }> = {};
        const LEVEL_SPACING = 350;
        const NODE_SPACING = 180;

        const maxNodesPerLevel = Math.max(...layout.map(l => l.length), 1);
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
            totalHeight: maxHeight + 200
        };
    }, [layout]);

    const handleReset = () => {
        x.set(0);
        y.set(0);
        setScale(1);
    };

    const zoomIn = () => setScale(s => Math.min(s + 0.1, 2));
    const zoomOut = () => setScale(s => Math.max(s - 0.1, 0.5));

    return (
        <div className={cn("relative w-full h-[600px] overflow-hidden bg-slate-950/50 backdrop-blur-md rounded-2xl border border-white/5 shadow-2xl group/graph", className)}>
            {/* Background Grid */}
            <div className="absolute inset-0 opacity-[0.03] pointer-events-none" style={{
                backgroundImage: `radial-gradient(circle at 2px 2px, white 1px, transparent 0)`,
                backgroundSize: '32px 32px'
            }} />

            {/* Controls */}
            <div className="absolute top-4 right-4 z-50 flex flex-col gap-2 opacity-0 group-hover/graph:opacity-100 transition-opacity duration-300">
                <div className="bg-black/40 backdrop-blur-md rounded-lg border border-white/10 p-1 flex flex-col gap-1">
                    <Button variant="ghost" size="icon" className="h-8 w-8 hover:bg-white/10 text-white/70" onClick={zoomIn}>
                        <Maximize2 className="h-4 w-4" />
                    </Button>
                    <Button variant="ghost" size="icon" className="h-8 w-8 hover:bg-white/10 text-white/70" onClick={zoomOut}>
                        <Minimize2 className="h-4 w-4" />
                    </Button>
                    <Button variant="ghost" size="icon" className="h-8 w-8 hover:bg-white/10 text-white/70" onClick={handleReset}>
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
                            <path d="M0,0 L10,5 L0,10 Z" fill="rgba(59, 130, 246, 0.5)" />
                        </marker>
                    </defs>

                    {graph.edges.map((edge, idx) => {
                        const from = nodePositions[edge.from];
                        const to = nodePositions[edge.to];
                        if (!from || !to) return null;

                        const fromNode = graph.nodes.find(n => n.id === edge.from);
                        const isActive = fromNode?.status === 'PROCESSING' || fromNode?.status === 'COMPLETED';

                        // Calculate points
                        const startX = from.x + 180;
                        const startY = from.y + 45;
                        const endX = to.x;
                        const endY = to.y + 45;
                        const midX = (startX + endX) / 2;
                        const pathString = `M ${startX} ${startY} C ${midX} ${startY}, ${midX} ${endY}, ${endX} ${endY}`;

                        return (
                            <g key={`${edge.from}-${edge.to}-${idx}`}>
                                {/* Base Path */}
                                <motion.path
                                    d={pathString}
                                    fill="none"
                                    stroke={isActive ? "rgba(59, 130, 246, 0.3)" : "rgba(255, 255, 255, 0.05)"}
                                    strokeWidth="2"
                                    initial={{ pathLength: 0 }}
                                    animate={{ pathLength: 1 }}
                                    transition={{ duration: 0.8, delay: idx * 0.1 }}
                                    markerEnd="url(#premium-arrowhead)"
                                />

                                {/* Flow Animation */}
                                {isActive && (
                                    <motion.path
                                        d={pathString}
                                        fill="none"
                                        stroke="url(#edgeGradient)"
                                        strokeWidth="3"
                                        strokeDasharray="10, 20"
                                        animate={{
                                            strokeDashoffset: [-60, 0]
                                        }}
                                        transition={{
                                            duration: 2,
                                            repeat: Infinity,
                                            ease: "linear"
                                        }}
                                    />
                                )}
                            </g>
                        );
                    })}
                </svg>

                {/* Nodes Layer */}
                <div className="relative">
                    {graph.nodes.map((node, idx) => {
                        const pos = nodePositions[node.id];
                        const config = STATUS_CONFIG[node.status];
                        const Icon = config.icon;

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
                                <div className={cn(
                                    "group relative p-4 rounded-2xl border bg-slate-900/40 backdrop-blur-xl transition-all duration-500 hover:scale-[1.02] hover:-translate-y-1",
                                    config.shadow,
                                    node.status === 'PROCESSING'
                                        ? "border-blue-500/50 shadow-[0_0_20px_rgba(59,130,246,0.2)]"
                                        : "border-white/10 hover:border-white/20"
                                )}>
                                    {/* Status Background Accent */}
                                    <div className={cn("absolute inset-0 rounded-2xl opacity-[0.03] group-hover:opacity-[0.07] transition-opacity", config.bg)} />

                                    <div className="flex items-start justify-between mb-3">
                                        <div className={cn(
                                            "p-2 rounded-xl transition-transform group-hover:scale-110 duration-300",
                                            config.bg
                                        )}>
                                            <Icon className={cn("h-4 w-4", config.color, config.animate && "animate-spin")} />
                                        </div>
                                        <Badge variant="outline" className="bg-white/5 border-white/10 text-[9px] uppercase tracking-widest font-bold opacity-70">
                                            {node.processor}
                                        </Badge>
                                    </div>

                                    <h4 className="text-sm font-bold truncate text-white/90 mb-1 tracking-tight">
                                        {node.label || node.id}
                                    </h4>

                                    <div className="flex items-center justify-between">
                                        <span className={cn("text-[10px] font-bold uppercase tracking-tighter", config.color)}>
                                            {node.status}
                                        </span>
                                        {node.job_id && (
                                            <span className="text-[9px] font-mono text-white/30">
                                                ID:{node.job_id.slice(0, 4)}
                                            </span>
                                        )}
                                    </div>

                                    {/* Animated progress bar for processing */}
                                    {node.status === 'PROCESSING' && (
                                        <div className="absolute bottom-0 left-0 right-0 h-1 overflow-hidden rounded-b-2xl">
                                            <motion.div
                                                className="h-full bg-blue-500"
                                                animate={{
                                                    x: ['-100%', '100%']
                                                }}
                                                transition={{
                                                    duration: 1.5,
                                                    repeat: Infinity,
                                                    ease: "easeInOut"
                                                }}
                                            />
                                        </div>
                                    )}
                                </div>
                            </motion.div>
                        );
                    })}
                </div>
            </motion.div>
        </div>
    );
}

function Badge({ children, className, variant }: { children: ReactNode, className?: string, variant?: string }) {
    return (
        <div className={cn(
            "inline-flex items-center rounded-full border px-2 py-0.5 text-[10px] font-bold transition-colors shadow-sm",
            variant === "outline" ? "border-white/10 text-white/70" : "bg-primary text-primary-foreground",
            className
        )}>
            {children}
        </div>
    );
}
