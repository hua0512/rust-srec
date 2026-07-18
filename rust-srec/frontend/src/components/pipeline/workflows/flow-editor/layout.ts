import type { Edge, Node, XYPosition } from '@xyflow/react';

const LEVEL_SPACING = 350;
const NODE_SPACING = 200;
const DETOUR_OFFSET = 180;
const PADDING = 50;

type Adjacency = {
  incoming: Map<string, Edge[]>;
  outgoing: Map<string, Edge[]>;
};

type DetourLane = {
  depth: number;
  direction: -1 | 1;
};

function buildAdjacency(edges: Edge[]): Adjacency {
  const outgoing = new Map<string, Edge[]>();
  const incoming = new Map<string, Edge[]>();

  for (const edge of edges) {
    const outgoingEdges = outgoing.get(edge.source);
    if (outgoingEdges) {
      outgoingEdges.push(edge);
    } else {
      outgoing.set(edge.source, [edge]);
    }

    const incomingEdges = incoming.get(edge.target);
    if (incomingEdges) {
      incomingEdges.push(edge);
    } else {
      incoming.set(edge.target, [edge]);
    }
  }

  return { incoming, outgoing };
}

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

function findDetourLanes(
  nodes: Node[],
  edges: Edge[],
  levels: Record<string, number>,
  { incoming, outgoing }: Adjacency,
): Map<string, DetourLane> {
  const detourLanes = new Map<string, DetourLane>();
  const longEdges = edges
    .map((edge, index) => ({ edge, index }))
    .filter(
      ({ edge }) => (levels[edge.target] ?? 0) - (levels[edge.source] ?? 0) > 1,
    )
    .sort((a, b) => {
      const aSpan = (levels[a.edge.target] ?? 0) - (levels[a.edge.source] ?? 0);
      const bSpan = (levels[b.edge.target] ?? 0) - (levels[b.edge.source] ?? 0);
      return bSpan - aSpan || a.index - b.index;
    });
  let independentDetourIndex = 0;

  for (const { edge } of longEdges) {
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

    const pathNodes = nodes.filter(
      (node) =>
        node.id !== edge.source &&
        node.id !== edge.target &&
        reachableFromSource.has(node.id) &&
        canReachTarget.has(node.id),
    );
    if (pathNodes.length === 0) continue;

    const inheritedLane = pathNodes.reduce<DetourLane | undefined>(
      (deepestLane, node) => {
        const lane = detourLanes.get(node.id);
        if (!lane || (deepestLane && deepestLane.depth >= lane.depth)) {
          return deepestLane;
        }
        return lane;
      },
      undefined,
    );
    const direction: -1 | 1 =
      inheritedLane?.direction ?? (independentDetourIndex % 2 === 0 ? -1 : 1);
    if (!inheritedLane) independentDetourIndex += 1;

    for (const node of pathNodes) {
      const existing = detourLanes.get(node.id);
      detourLanes.set(node.id, {
        depth: (existing?.depth ?? 0) + 1,
        direction: existing?.direction ?? direction,
      });
    }
  }

  return detourLanes;
}

function verticalOrder(nodesByLevel: Node[][]): Map<string, number> {
  const order = new Map<string, number>();

  for (const levelNodes of nodesByLevel) {
    if (!levelNodes) continue;

    const centerIndex = (levelNodes.length - 1) / 2;
    levelNodes.forEach((node, index) => {
      order.set(node.id, index - centerIndex);
    });
  }

  return order;
}

function neighborBarycenter(
  nodeId: string,
  adjacency: Map<string, Edge[]>,
  order: Map<string, number>,
): number | undefined {
  const positions = (adjacency.get(nodeId) ?? [])
    .map((edge) =>
      order.get(edge.source === nodeId ? edge.target : edge.source),
    )
    .filter((position): position is number => position !== undefined);

  if (positions.length === 0) return undefined;
  return (
    positions.reduce((sum, position) => sum + position, 0) / positions.length
  );
}

function orderLevel(
  levelNodes: Node[],
  adjacency: Map<string, Edge[]>,
  order: Map<string, number>,
  originalOrder: Map<string, number>,
): Node[] {
  return [...levelNodes].sort((a, b) => {
    const aCenter = neighborBarycenter(a.id, adjacency, order);
    const bCenter = neighborBarycenter(b.id, adjacency, order);

    if (aCenter !== undefined && bCenter !== undefined && aCenter !== bCenter) {
      return aCenter - bCenter;
    }
    if (aCenter !== undefined && bCenter === undefined) return -1;
    if (aCenter === undefined && bCenter !== undefined) return 1;
    return (originalOrder.get(a.id) ?? 0) - (originalOrder.get(b.id) ?? 0);
  });
}

function minimizeCrossings(
  nodesByLevel: Node[][],
  { incoming, outgoing }: Adjacency,
  originalOrder: Map<string, number>,
) {
  for (let pass = 0; pass < 2; pass += 1) {
    let order = verticalOrder(nodesByLevel);
    for (let level = 1; level < nodesByLevel.length; level += 1) {
      const levelNodes = nodesByLevel[level];
      if (levelNodes) {
        nodesByLevel[level] = orderLevel(
          levelNodes,
          incoming,
          order,
          originalOrder,
        );
        order = verticalOrder(nodesByLevel);
      }
    }

    order = verticalOrder(nodesByLevel);
    for (let level = nodesByLevel.length - 2; level >= 0; level -= 1) {
      const levelNodes = nodesByLevel[level];
      if (levelNodes) {
        nodesByLevel[level] = orderLevel(
          levelNodes,
          outgoing,
          order,
          originalOrder,
        );
        order = verticalOrder(nodesByLevel);
      }
    }
  }
}

export function getInitialNodePosition(
  nodes: Node[],
  dependencyIds: string[],
): XYPosition {
  if (nodes.length === 0) return { x: PADDING, y: PADDING };

  const dependencySet = new Set(dependencyIds);
  const dependencies = nodes.filter((node) => dependencySet.has(node.id));
  if (dependencies.length > 0) {
    const x =
      Math.max(...dependencies.map((node) => node.position.x)) + LEVEL_SPACING;
    let y =
      dependencies.reduce((sum, node) => sum + node.position.y, 0) /
      dependencies.length;

    const nodesAtLevel = nodes.filter(
      (node) => Math.abs(node.position.x - x) < LEVEL_SPACING / 2,
    );
    while (
      nodesAtLevel.some((node) => Math.abs(node.position.y - y) < NODE_SPACING)
    ) {
      y += NODE_SPACING;
    }

    return {
      x,
      y,
    };
  }

  const minX = Math.min(...nodes.map((node) => node.position.x));
  const firstLevelNodes = nodes.filter((node) => node.position.x === minX);
  return {
    x: minX,
    y:
      Math.max(...firstLevelNodes.map((node) => node.position.y)) +
      NODE_SPACING,
  };
}

export function getLayoutedElements(nodes: Node[], edges: Edge[]) {
  const levels: Record<string, number> = {};
  const nodeMap: Record<string, Node> = {};
  const originalOrder = new Map<string, number>();
  nodes.forEach((node, index) => {
    nodeMap[node.id] = node;
    originalOrder.set(node.id, index);
  });

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

  const adjacency = buildAdjacency(edges);
  minimizeCrossings(nodesByLevel, adjacency, originalOrder);

  const detourLanes = findDetourLanes(nodes, edges, levels, adjacency);
  const yByNodeId = new Map<string, number>();

  for (const levelNodes of nodesByLevel) {
    if (!levelNodes) continue;

    const centerIndex = (levelNodes.length - 1) / 2;
    levelNodes.forEach((node, nodeIndex) => {
      let y = (nodeIndex - centerIndex) * NODE_SPACING;

      const detourLane = detourLanes.get(node.id);
      if (detourLane) {
        y += detourLane.direction * detourLane.depth * DETOUR_OFFSET;
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
