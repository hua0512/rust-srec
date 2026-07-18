import type { Edge, Node } from '@xyflow/react';
import { describe, expect, it } from 'vitest';

import { getLayoutedElements } from './layout';

function node(id: string): Node {
  return { id, position: { x: 0, y: 0 }, data: {} };
}

function edge(source: string, target: string): Edge {
  return { id: `${source}-${target}`, source, target };
}

function positionsById(nodes: Node[]) {
  return new Map(nodes.map((item) => [item.id, item.position]));
}

describe('workflow graph layout', () => {
  it('keeps a simple chain on one lane', () => {
    const result = getLayoutedElements(
      [node('a'), node('b'), node('c')],
      [edge('a', 'b'), edge('b', 'c')],
    );
    const positions = positionsById(result.nodes);

    expect(positions.get('a')?.y).toBe(positions.get('b')?.y);
    expect(positions.get('b')?.y).toBe(positions.get('c')?.y);
    expect(positions.get('a')?.x).toBeLessThan(positions.get('b')?.x ?? 0);
    expect(positions.get('b')?.x).toBeLessThan(positions.get('c')?.x ?? 0);
  });

  it('separates a bypassed path from its direct edge', () => {
    const result = getLayoutedElements(
      [node('remux'), node('thumbnail'), node('upload')],
      [
        edge('remux', 'thumbnail'),
        edge('thumbnail', 'upload'),
        edge('remux', 'upload'),
      ],
    );
    const positions = positionsById(result.nodes);

    expect(positions.get('remux')?.y).toBe(positions.get('upload')?.y);
    expect(positions.get('thumbnail')?.y).toBeLessThan(
      positions.get('remux')?.y ?? 0,
    );
  });

  it('places parallel branches on separate lanes around their join', () => {
    const result = getLayoutedElements(
      [node('source'), node('video'), node('image'), node('upload')],
      [
        edge('source', 'video'),
        edge('source', 'image'),
        edge('video', 'upload'),
        edge('image', 'upload'),
      ],
    );
    const positions = positionsById(result.nodes);

    expect(positions.get('source')?.y).toBe(positions.get('upload')?.y);
    expect(positions.get('video')?.y).not.toBe(positions.get('image')?.y);
  });

  it('uses deeper lanes for nested bypass relationships', () => {
    const result = getLayoutedElements(
      [node('a'), node('b'), node('c'), node('d')],
      [
        edge('a', 'b'),
        edge('b', 'c'),
        edge('c', 'd'),
        edge('a', 'd'),
        edge('b', 'd'),
      ],
    );
    const positions = positionsById(result.nodes);

    expect(positions.get('a')?.y).toBe(positions.get('d')?.y);
    expect(positions.get('b')?.y).toBeLessThan(positions.get('a')?.y ?? 0);
    expect(positions.get('c')?.y).toBeLessThan(positions.get('b')?.y ?? 0);
  });
});
