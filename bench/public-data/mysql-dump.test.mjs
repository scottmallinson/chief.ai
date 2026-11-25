import { expect, test } from 'vitest';

import { parseValues } from './mysql-dump.mjs';

test('reads numbers, NULL and strings, several tuples to a statement', () => {
  expect(parseValues("(1,NULL,'a'),(2,3.5,'b');")).toEqual([
    [1, null, 'a'],
    [2, 3.5, 'b'],
  ]);
});

test('a comma, bracket or semicolon inside a string is part of the string', () => {
  expect(parseValues("(1,'a,b),(c;d');")).toEqual([[1, 'a,b),(c;d']]);
});

test('reads the escapes mysqldump writes', () => {
  expect(parseValues("(1,'it\\'s','x\\ny','say \\\"hi\\\"','back\\\\slash','it''s');")).toEqual([
    [1, "it's", 'x\ny', 'say "hi"', 'back\\slash', "it's"],
  ]);
});

test('refuses what it does not understand rather than guessing', () => {
  expect(() => parseValues('(1,abc)')).toThrow(/not a number/);
  expect(() => parseValues("(1,'open")).toThrow(/unterminated/);
  expect(() => parseValues('1,2')).toThrow(/expected "\("/);
});
