import type { Edge, Node } from '@xyflow/react';
import { describe, expect, it } from 'vitest';

import { getInitialNodePosition, getLayoutedElements } from './layout';

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

  it('orders connected levels to reduce edge crossings', () => {
    const result = getLayoutedElements(
      [node('a'), node('b'), node('x'), node('y')],
      [edge('a', 'y'), edge('b', 'x')],
    );
    const positions = positionsById(result.nodes);

    expect(positions.get('y')?.y).toBeLessThan(positions.get('x')?.y ?? 0);
  });

  it('balances independent detours above and below their direct paths', () => {
    const result = getLayoutedElements(
      [
        node('a'),
        node('b'),
        node('c'),
        node('d'),
        node('e'),
        node('f'),
        node('g'),
      ],
      [
        edge('a', 'b'),
        edge('b', 'c'),
        edge('c', 'd'),
        edge('a', 'd'),
        edge('b', 'd'),
        edge('e', 'f'),
        edge('f', 'g'),
        edge('e', 'g'),
      ],
    );
    const positions = positionsById(result.nodes);

    expect(positions.get('b')?.y).toBeLessThan(positions.get('a')?.y ?? 0);
    expect(positions.get('f')?.y).toBeGreaterThan(positions.get('e')?.y ?? 0);
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

  it('places a new child to the right at its dependencies average height', () => {
    const existingNodes = [
      { ...node('a'), position: { x: 50, y: 50 } },
      { ...node('b'), position: { x: 50, y: 250 } },
    ];

    expect(getInitialNodePosition(existingNodes, ['a', 'b'])).toEqual({
      x: 400,
      y: 150,
    });
  });

  it('places a new root below existing first-level roots', () => {
    const existingNodes = [
      { ...node('a'), position: { x: 50, y: 50 } },
      { ...node('b'), position: { x: 50, y: 250 } },
      { ...node('c'), position: { x: 400, y: 50 } },
    ];

    expect(getInitialNodePosition(existingNodes, [])).toEqual({
      x: 50,
      y: 450,
    });
  });

  it('avoids an occupied position when adding a sibling', () => {
    const existingNodes = [
      { ...node('source'), position: { x: 50, y: 50 } },
      { ...node('first-child'), position: { x: 400, y: 50 } },
    ];

    expect(getInitialNodePosition(existingNodes, ['source'])).toEqual({
      x: 400,
      y: 250,
    });
  });
});
