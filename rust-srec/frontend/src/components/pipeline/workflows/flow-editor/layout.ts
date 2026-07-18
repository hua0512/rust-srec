import type { Edge, Node } from '@xyflow/react';

const LEVEL_SPACING = 350;
const NODE_SPACING = 200;
const DETOUR_OFFSET = 180;
const PADDING = 50;

function collectReachable(
  startId: string,
  adjacency: Map<string, Edge[]>,
  excludedEdgeId: string,
): Set<string> {
  const reachable = new Set<string>();
  const pending = [startId];

  while (pending.length > 0) {
    const current = pending.pop();
    if (!current || reachable.has(current)) continue;

    reachable.add(current);
    for (const edge of adjacency.get(current) ?? []) {
      if (edge.id !== excludedEdgeId) {
        pending.push(edge.source === current ? edge.target : edge.source);
      }
    }
  }

  return reachable;
}

// Separate alternate multi-step paths from direct long edges so neither route is hidden.
function findDetourDepths(
  nodes: Node[],
  edges: Edge[],
  levels: Record<string, number>,
): Map<string, number> {
  const outgoing = new Map<string, Edge[]>();
  const incoming = new Map<string, Edge[]>();

  for (const edge of edges) {
    outgoing.set(edge.source, [...(outgoing.get(edge.source) ?? []), edge]);
    incoming.set(edge.target, [...(incoming.get(edge.target) ?? []), edge]);
  }

  const detourDepths = new Map<string, number>();

  for (const edge of edges) {
    const sourceLevel = levels[edge.source];
    const targetLevel = levels[edge.target];
    if (
      sourceLevel === undefined ||
      targetLevel === undefined ||
      targetLevel - sourceLevel <= 1
    ) {
      continue;
    }

    const reachableFromSource = collectReachable(
      edge.source,
      outgoing,
      edge.id,
    );
    const canReachTarget = collectReachable(edge.target, incoming, edge.id);

    for (const node of nodes) {
      if (
        node.id !== edge.source &&
        node.id !== edge.target &&
        reachableFromSource.has(node.id) &&
        canReachTarget.has(node.id)
      ) {
        detourDepths.set(node.id, (detourDepths.get(node.id) ?? 0) + 1);
      }
    }
  }

  return detourDepths;
}

export function getLayoutedElements(nodes: Node[], edges: Edge[]) {
  const levels: Record<string, number> = {};
  const nodeMap: Record<string, Node> = {};
  nodes.forEach((n) => (nodeMap[n.id] = n));

  const getLevel = (id: string, visiting = new Set<string>()): number => {
    if (levels[id] !== undefined) return levels[id];
    if (visiting.has(id)) return 0;

    const nextVisiting = new Set(visiting);
    nextVisiting.add(id);

    const incoming = edges.filter(
      (edge) => edge.target === id && nodeMap[edge.source],
    );
    if (incoming.length === 0) {
      levels[id] = 0;
      return 0;
    }

    const maxLevel = Math.max(
      ...incoming.map((edge) => getLevel(edge.source, nextVisiting)),
      -1,
    );
    levels[id] = maxLevel + 1;
    return levels[id];
  };

  nodes.forEach((n) => getLevel(n.id));

  const nodesByLevel: Node[][] = [];
  nodes.forEach((node) => {
    const level = levels[node.id];
    if (!nodesByLevel[level]) nodesByLevel[level] = [];
    nodesByLevel[level].push(node);
  });

  const detourDepths = findDetourDepths(nodes, edges, levels);
  const yByNodeId = new Map<string, number>();

  for (const levelNodes of nodesByLevel) {
    if (!levelNodes) continue;

    const centerIndex = (levelNodes.length - 1) / 2;
    levelNodes.forEach((node, nodeIndex) => {
      let y = (nodeIndex - centerIndex) * NODE_SPACING;

      if (levelNodes.length === 1) {
        y -= (detourDepths.get(node.id) ?? 0) * DETOUR_OFFSET;
      }

      yByNodeId.set(node.id, y);
    });
  }

  const minY = Math.min(...yByNodeId.values(), 0);
  const yShift = PADDING - minY;

  const layoutedNodes = nodes.map((node) => {
    const level = levels[node.id];

    return {
      ...node,
      position: {
        x: level * LEVEL_SPACING + PADDING,
        y: (yByNodeId.get(node.id) ?? 0) + yShift,
      },
    };
  });

  return { nodes: layoutedNodes, edges };
}
