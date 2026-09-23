import { computeNodeLevels } from '../graph-levels';

const edge = (source: string, target: string) => ({ source, target });

describe('computeNodeLevels', () => {
  it('places each node at its longest path from a root', () => {
    expect(
      computeNodeLevels(
        ['a', 'b', 'c', 'd'],
        [edge('a', 'b'), edge('b', 'c'), edge('a', 'c'), edge('c', 'd')],
      ),
    ).toEqual({ a: 0, b: 1, c: 2, d: 3 });
  });

  it('ignores edges from unknown nodes', () => {
    expect(computeNodeLevels(['a', 'b'], [edge('ghost', 'b')])).toEqual({
      a: 0,
      b: 0,
    });
  });

  it('terminates on a cycle', () => {
    const levels = computeNodeLevels(
      ['a', 'b'],
      [edge('a', 'b'), edge('b', 'a')],
    );
    expect(Object.keys(levels).sort()).toEqual(['a', 'b']);
  });
});
