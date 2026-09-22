import { getPaginationPages } from '../pagination';

it.each<[number, number, (number | 'ellipsis')[]]>([
  [0, 0, []],
  [1, 0, [0]],
  [7, 3, [0, 1, 2, 3, 4, 5, 6]],
  [10, 0, [0, 1, 'ellipsis', 9]],
  [10, 2, [0, 1, 2, 3, 'ellipsis', 9]],
  [10, 3, [0, 'ellipsis', 2, 3, 4, 'ellipsis', 9]],
  [10, 6, [0, 'ellipsis', 5, 6, 7, 'ellipsis', 9]],
  [10, 7, [0, 'ellipsis', 6, 7, 8, 9]],
  [10, 9, [0, 'ellipsis', 8, 9]],
])('lists %i pages around index %i', (totalPages, currentPage, expected) => {
  expect(getPaginationPages(totalPages, currentPage)).toEqual(expected);
});
