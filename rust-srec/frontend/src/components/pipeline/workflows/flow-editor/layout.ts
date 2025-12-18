import { Node, Edge } from '@xyflow/react';

export function getLayoutedElements(nodes: Node[], edges: Edge[]) {
    const LEVEL_SPACING = 350;
    const NODE_SPACING = 200;

    const levels: Record<string, number> = {};
    const nodeMap: Record<string, Node> = {};
    nodes.forEach(n => nodeMap[n.id] = n);

    const getLevel = (id: string, visited = new Set<string>()): number => {
        if (levels[id] !== undefined) return levels[id];
        if (visited.has(id)) return 0; // Cycle safety
        visited.add(id);

        const incoming = edges.filter(e => e.target === id);
        if (incoming.length === 0) {
            levels[id] = 0;
            return 0;
        }

        const maxLevel = Math.max(...incoming.map(e => getLevel(e.source, visited)), -1);
        levels[id] = maxLevel + 1;
        return levels[id];
    };

    nodes.forEach(n => getLevel(n.id));

    const nodesByLevel: Node[][] = [];
    Object.entries(levels).forEach(([id, level]) => {
        if (!nodesByLevel[level]) nodesByLevel[level] = [];
        nodesByLevel[level].push(nodeMap[id]);
    });

    const maxNodesPerLevel = Math.max(...nodesByLevel.map(l => l.length), 1);
    const maxHeight = maxNodesPerLevel * NODE_SPACING;

    const layoutedNodes = nodes.map(node => {
        const level = levels[node.id];
        const levelNodes = nodesByLevel[level];
        const nodeIndex = levelNodes.findIndex(n => n.id === node.id);

        const levelHeight = (levelNodes.length - 1) * NODE_SPACING;
        const yOffset = (maxHeight - levelHeight) / 2;

        return {
            ...node,
            position: {
                x: level * LEVEL_SPACING + 50,
                y: yOffset + nodeIndex * NODE_SPACING + 50,
            },
        };
    });

    return { nodes: layoutedNodes, edges };
}
