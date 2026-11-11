export interface Row {
  id: string;
  /** Shown and matched, including its leading "/" or "@". */
  name: string;
  description: string;
  source: string;
  group: string;
}

/** Offsets in `name` where a word starts: the beginning and after each separator. */
function wordStarts(name: string): number[] {
  const starts = [0];
  const sep = /[:\-_\s]+/g;
  for (let m = sep.exec(name); m; m = sep.exec(name)) starts.push(m.index + m[0].length);
  return starts;
}

function rank(query: string, row: Row): number | null {
  const name = row.name.replace(/^[/@]/, "").toLowerCase();
  if (wordStarts(name).some((i) => name.startsWith(query, i))) return 0;
  const words = row.description.toLowerCase().split(/[^\p{L}\p{N}]+/u);
  if (words.some((w) => w.startsWith(query))) return 1;
  return null;
}

/**
 * Rows whose name has a word starting with `query` first, then rows whose
 * description does; original order within each rank. Name words are split on
 * ":", "-" and "_".
 */
export function matchRows(query: string, rows: Row[]): Row[] {
  const q = query.trim().replace(/^[/@]/, "").toLowerCase();
  if (q === "") return rows;
  const ranked: Row[][] = [[], []];
  for (const row of rows) {
    const r = rank(q, row);
    if (r !== null) ranked[r].push(row);
  }
  return ranked.flat();
}
