/**
 * Assigns every node its longest-path depth from a root (a node with no incoming edges from a
 * known node). Edges from unknown nodes are ignored, and a cycle is cut where it closes, so the
 * result is defined for any input.
 */
export function computeNodeLevels(
  nodeIds: readonly string[],
  edges: readonly { source: string; target: string }[],
): Record<string, number> {
  const levels: Record<string, number> = {};
  const known = new Set(nodeIds);

  const getLevel = (id: string, visiting = new Set<string>()): number => {
    if (levels[id] !== undefined) return levels[id];
    if (visiting.has(id)) return 0;

    const nextVisiting = new Set(visiting);
    nextVisiting.add(id);

    const incoming = edges.filter(
      (edge) => edge.target === id && known.has(edge.source),
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

  nodeIds.forEach((id) => getLevel(id));
  return levels;
}
