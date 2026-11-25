// Reads the INSERT statements of a mysqldump file, one table at a time, without holding the file in
// memory. The TAWOS dump is 4 GB, so this is a streaming tokenizer rather than a loaded string.
//
// It understands exactly what mysqldump writes: `INSERT INTO `T` VALUES (..),(..);` with numbers,
// NULL and single-quoted strings escaped with a backslash. It is not a SQL parser and refuses
// anything else it meets inside a values list.

import { createReadStream } from 'node:fs';

const ESCAPES = { n: '\n', r: '\r', t: '\t', 0: '\0', b: '\b', Z: '\x1a' };

/** Parse one `(a,b,'c')` tuple list from `text`, which is the part after VALUES. */
export function parseValues(text) {
  const out = [];
  let i = 0;
  const n = text.length;
  while (i < n) {
    while (i < n && (text[i] === ',' || text[i] === ' ')) i++;
    if (i >= n || text[i] === ';') break;
    if (text[i] !== '(') throw new Error(`expected "(" at ${i}, found ${JSON.stringify(text[i])}`);
    i++;
    const row = [];
    for (;;) {
      const c = text[i];
      if (c === "'") {
        i++;
        let s = '';
        let from = i;
        for (;;) {
          const q = text[i];
          if (q === undefined) throw new Error('unterminated string');
          if (q === '\\') {
            s += text.slice(from, i) + (ESCAPES[text[i + 1]] ?? text[i + 1]);
            i += 2;
            from = i;
          } else if (q === "'") {
            if (text[i + 1] === "'") {
              s += text.slice(from, i) + "'";
              i += 2;
              from = i;
            } else {
              s += text.slice(from, i);
              i++;
              break;
            }
          } else i++;
        }
        row.push(s);
      } else {
        let j = i;
        while (text[j] !== ',' && text[j] !== ')') {
          if (text[j] === undefined) throw new Error('unterminated tuple');
          j++;
        }
        const raw = text.slice(i, j);
        row.push(raw === 'NULL' ? null : Number(raw));
        if (raw !== 'NULL' && Number.isNaN(row.at(-1))) throw new Error(`not a number: ${raw}`);
        i = j;
      }
      if (text[i] === ',') {
        i++;
        continue;
      }
      if (text[i] === ')') {
        i++;
        break;
      }
      throw new Error(`unexpected ${JSON.stringify(text[i])} at ${i}`);
    }
    out.push(row);
  }
  return out;
}

/** Yield every row of `table`, as an array of values in the table's column order. */
export async function* rows(path, table) {
  const prefix = `INSERT INTO \`${table}\` VALUES `;
  let tail = '';
  for await (const chunk of createReadStream(path, { encoding: 'utf8', highWaterMark: 1 << 24 })) {
    const lines = (tail + chunk).split('\n');
    tail = lines.pop();
    for (const line of lines)
      if (line.startsWith(prefix)) yield* parseValues(line.slice(prefix.length));
  }
  if (tail.startsWith(prefix)) yield* parseValues(tail.slice(prefix.length));
}
