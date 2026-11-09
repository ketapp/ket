// An agent's words, read as markdown: marked's lexer, the way GitHub reads
// it — emphasis that nests and wraps code, escapes, tables, task lists,
// strikethrough, autolinks — turned into the few blocks and runs the chat
// draws. Nothing here draws; see `Words` in the session screen.

import { type Token, type Tokens, lexer } from 'marked';

/** A run of text inside a block. */
export type Run =
  | { kind: 'text'; text: string }
  | { kind: 'code'; text: string }
  | { kind: 'strong' | 'em' | 'struck'; runs: Run[] }
  /** Tappable: a web or mail address. A link to anything else — a path in
   * the repository, say — is only its words. */
  | { kind: 'link'; url: string; runs: Run[] }
  | { kind: 'break' };

export type Align = 'left' | 'center' | 'right';

/** One block of an agent's words. A list is read out into its items, each
 * with how deep it sits. */
export type Block =
  | { kind: 'heading'; level: number; runs: Run[] }
  | { kind: 'para'; runs: Run[] }
  | { kind: 'item'; depth: number; marker: string; runs: Run[] }
  | { kind: 'task'; depth: number; done: boolean; runs: Run[] }
  | { kind: 'quote'; blocks: Block[] }
  | { kind: 'rule' }
  | { kind: 'code'; text: string }
  | { kind: 'table'; head: Run[][]; align: Align[]; rows: Run[][][] };

/** Words read lately, by their text: a chat draws the same turns again with
 * every update, and a long one is worth reading once. */
const read = new Map<string, Block[]>();
const KEPT = 300;

/** `text` as the chat draws it. */
export function markdown(text: string): Block[] {
  const kept = read.get(text);
  if (kept) return kept;
  const blocks = blocksOf(lexer(text, { gfm: true }));
  if (read.size >= KEPT) read.delete(read.keys().next().value as string);
  read.set(text, blocks);
  return blocks;
}

function blocksOf(tokens: Token[]): Block[] {
  const out: Block[] = [];
  for (const token of tokens) block(token, out);
  return out;
}

function block(token: Token, out: Block[]): void {
  switch (token.type) {
    case 'heading':
      out.push({ kind: 'heading', level: token.depth, runs: runsOf(token.tokens) });
      return;
    case 'paragraph':
      out.push({ kind: 'para', runs: runsOf(token.tokens) });
      return;
    case 'text':
      out.push({ kind: 'para', runs: token.tokens ? runsOf(token.tokens) : [{ kind: 'text', text: decode(token.text) }] });
      return;
    case 'list':
      list(token as Tokens.List, 0, out);
      return;
    case 'blockquote':
      out.push({ kind: 'quote', blocks: blocksOf(token.tokens ?? []) });
      return;
    case 'hr':
      out.push({ kind: 'rule' });
      return;
    case 'code':
      out.push({ kind: 'code', text: token.text });
      return;
    case 'table': {
      const table = token as Tokens.Table;
      out.push({
        kind: 'table',
        head: table.header.map((cell) => runsOf(cell.tokens)),
        align: table.align.map((align) => align ?? 'left'),
        rows: table.rows.map((row) => row.map((cell) => runsOf(cell.tokens))),
      });
      return;
    }
    case 'html': {
      // A block of HTML — `<details>`, say — as the words in it.
      const words = decode(token.text.replace(/<br\s*\/?>/gi, '\n').replace(/<[^>]*>/g, '')).trim();
      if (words) out.push({ kind: 'para', runs: [{ kind: 'text', text: words }] });
      return;
    }
    default:
      // Blank lines and link definitions say nothing of their own.
      return;
  }
}

/** A list's items, one deeper for each list inside one. An item's further
 * paragraphs join its first; a code block or a table in it follows it. */
function list(token: Tokens.List, depth: number, out: Block[]): void {
  const start = typeof token.start === 'number' ? token.start : 1;
  token.items.forEach((item, at) => {
    const runs: Run[] = [];
    const after: Block[] = [];
    for (const part of item.tokens) {
      if (part.type === 'checkbox') continue;
      if (part.type === 'list') {
        after.push(...listed(part as Tokens.List, depth + 1));
      } else if (part.type === 'text' || part.type === 'paragraph') {
        if (runs.length > 0) runs.push({ kind: 'break' }, { kind: 'break' });
        runs.push(...(part.tokens ? runsOf(part.tokens) : [{ kind: 'text' as const, text: decode(part.text) }]));
      } else {
        block(part, after);
      }
    }
    out.push(
      item.task
        ? { kind: 'task', depth, done: item.checked === true, runs }
        : { kind: 'item', depth, marker: token.ordered ? `${start + at}.` : '•', runs },
    );
    out.push(...after);
  });
}

function listed(token: Tokens.List, depth: number): Block[] {
  const out: Block[] = [];
  list(token, depth, out);
  return out;
}

function runsOf(tokens: Token[] | undefined): Run[] {
  const out: Run[] = [];
  for (const token of tokens ?? []) run(token, out);
  return out;
}

function run(token: Token, out: Run[]): void {
  switch (token.type) {
    case 'text':
      if (token.tokens) out.push(...runsOf(token.tokens));
      else out.push({ kind: 'text', text: decode(token.text) });
      return;
    case 'escape':
      out.push({ kind: 'text', text: token.text });
      return;
    case 'codespan':
      out.push({ kind: 'code', text: decode(token.text) });
      return;
    case 'strong':
      out.push({ kind: 'strong', runs: runsOf(token.tokens) });
      return;
    case 'em':
      out.push({ kind: 'em', runs: runsOf(token.tokens) });
      return;
    case 'del':
      out.push({ kind: 'struck', runs: runsOf(token.tokens) });
      return;
    case 'link':
    case 'image': {
      // An image is its words, linked to it: the chat draws no pictures.
      const runs = runsOf(token.tokens);
      const words = runs.length > 0 ? runs : [{ kind: 'text' as const, text: token.text || token.href }];
      if (/^(https?:|mailto:)/i.test(token.href)) out.push({ kind: 'link', url: token.href, runs: words });
      else out.push(...words);
      return;
    }
    case 'br':
      out.push({ kind: 'break' });
      return;
    case 'html':
      // `<br>` breaks the line, as agents use it in table cells; any other
      // tag goes, and the words between tags stay.
      if (/^<br\s*\/?>$/i.test(token.text.trim())) out.push({ kind: 'break' });
      return;
    default:
      if ('text' in token && typeof token.text === 'string') out.push({ kind: 'text', text: decode(token.text) });
  }
}

/** The entities marked leaves in text: the five it writes itself, the ones
 * agents use, and any by number. */
const ENTITIES: Record<string, string> = {
  amp: '&',
  lt: '<',
  gt: '>',
  quot: '"',
  apos: "'",
  nbsp: ' ',
  copy: '©',
  reg: '®',
  trade: '™',
  mdash: '—',
  ndash: '–',
  hellip: '…',
  lsquo: '‘',
  rsquo: '’',
  ldquo: '“',
  rdquo: '”',
  times: '×',
  rarr: '→',
  larr: '←',
};

function decode(text: string): string {
  return text.replace(/&(#x[0-9a-f]+|#\d+|[a-z]+);/gi, (whole, name: string) => {
    if (name[0] === '#') {
      const code = name[1] === 'x' || name[1] === 'X' ? parseInt(name.slice(2), 16) : parseInt(name.slice(1), 10);
      return code > 0 && code <= 0x10ffff ? String.fromCodePoint(code) : whole;
    }
    return ENTITIES[name.toLowerCase()] ?? whole;
  });
}
