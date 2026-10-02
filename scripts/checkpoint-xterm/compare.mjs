// The xterm.js half of the Epic 5b.0 spike.
//
// `cargo run --release -p ket-core --example checkpoint -- all <dir>` writes a
// `<scenario>.xterm.json` per recording: the whole byte stream, a sample of
// checkpoints with the output after each, and alacritty's final screen as
// text. For every checkpoint this rebuilds a headless xterm.js from the
// checkpoint plus the rest, and compares it with two references:
//
// - xterm.js fed the raw stream from the start. A difference here is the
//   checkpoint's fault: something xterm.js reads differently when it arrives
//   as a checkpoint rather than as the program wrote it.
// - alacritty's final screen, as text. A difference here is what a phone
//   would actually show differently from the desktop, whoever is at fault.
//
// Usage: npm install && node compare.mjs <dir>

import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import headless from '@xterm/headless';

const { Terminal } = headless;

const dir = process.argv[2];
if (!dir) {
  console.error('usage: node compare.mjs <dir written by the checkpoint example>');
  process.exit(1);
}

const fromHex = (hex) => Uint8Array.from(Buffer.from(hex, 'hex'));

function open(meta) {
  return new Terminal({
    cols: meta.cols,
    rows: meta.rows,
    scrollback: meta.scrollback,
    allowProposedApi: true,
  });
}

const write = (term, bytes) => new Promise((done) => term.write(bytes, done));

/// Everything comparable about the active buffer: per line, the text and a
/// signature of every cell's attributes; and the cursor.
function dump(term) {
  const buffer = term.buffer.active;
  const lines = [];
  const cell = buffer.getNullCell();
  for (let y = 0; y < buffer.length; y++) {
    const line = buffer.getLine(y);
    const attrs = [];
    for (let x = 0; x < term.cols; x++) {
      line.getCell(x, cell);
      attrs.push(
        [
          // A cell nothing was ever written to reads as '' in xterm.js and
          // as ' ' once a checkpoint has written a blank into it. Alacritty
          // stores both as ' ', so a checkpoint cannot tell them apart; on
          // screen they are the same.
          cell.getChars() || ' ',
          cell.getWidth(),
          cell.isFgDefault() ? 'd' : cell.isFgRGB() ? `r${cell.getFgColor()}` : `p${cell.getFgColor()}`,
          cell.isBgDefault() ? 'd' : cell.isBgRGB() ? `r${cell.getBgColor()}` : `p${cell.getBgColor()}`,
          cell.isBold(),
          cell.isItalic(),
          cell.isDim(),
          cell.isUnderline(),
          cell.isInverse(),
          cell.isInvisible(),
          cell.isStrikethrough(),
        ].join(','),
      );
    }
    lines.push({ text: line.translateToString(true), attrs: attrs.join('|'), wrapped: line.isWrapped });
  }
  return {
    type: buffer.type,
    lines,
    cursor: [buffer.cursorY, buffer.cursorX],
    base: buffer.baseY,
  };
}

/// The first few differences between two dumps, lined up from the bottom:
/// the screens must agree exactly, the scrollback as far as both kept it.
function differences(a, b) {
  const out = [];
  if (a.type !== b.type) out.push(`buffer ${a.type} vs ${b.type}`);
  if (a.cursor.join() !== b.cursor.join()) out.push(`cursor ${a.cursor} vs ${b.cursor}`);
  const n = Math.min(a.lines.length, b.lines.length);
  if (a.lines.length !== b.lines.length) out.push(`${a.lines.length} lines vs ${b.lines.length}`);
  let bad = 0;
  for (let i = 1; i <= n; i++) {
    const la = a.lines[a.lines.length - i];
    const lb = b.lines[b.lines.length - i];
    const what =
      la.text !== lb.text ? 'text' : la.attrs !== lb.attrs ? 'attributes' : la.wrapped !== lb.wrapped ? 'wrap' : null;
    if (what) {
      bad++;
      if (bad <= 2) {
        let detail = '';
        if (what === 'attributes') {
          const ca = la.attrs.split('|');
          const cb = lb.attrs.split('|');
          const x = ca.findIndex((v, k) => v !== cb[k]);
          detail = ` at col ${x}: [${ca[x]}] vs [${cb[x]}]`;
        }
        out.push(`line -${i} ${what}${detail}: ${JSON.stringify(la.text)} vs ${JSON.stringify(lb.text)}`);
      }
    }
  }
  if (bad > 2) out.push(`${bad} lines differ in all`);
  return out;
}

/// The text differences against alacritty, lined up from the bottom.
function textDifferences(alacritty, xterm) {
  const out = [];
  const n = Math.min(alacritty.length, xterm.lines.length);
  let bad = 0;
  for (let i = 1; i <= n; i++) {
    const a = alacritty[alacritty.length - i].trimEnd();
    const b = xterm.lines[xterm.lines.length - i].text.trimEnd();
    if (a !== b) {
      bad++;
      if (bad <= 2) out.push(`line -${i}: ${JSON.stringify(a)} vs ${JSON.stringify(b)}`);
    }
  }
  if (bad > 2) out.push(`${bad} lines differ in all`);
  return out;
}

const files = readdirSync(dir).filter((f) => f.endsWith('.xterm.json')).sort();
const summary = [];
for (const file of files) {
  const data = JSON.parse(readFileSync(join(dir, file), 'utf8'));
  const stream = fromHex(data.stream);

  const reference = open(data);
  await write(reference, stream);
  const expected = dump(reference);
  const baseline = textDifferences(data.alacritty, expected);

  let checkpointBad = 0;
  let alacrittyBad = 0;
  const examples = [];
  for (const c of data.cases) {
    const term = open(data);
    await write(term, fromHex(c.checkpoint));
    await write(term, stream.subarray(c.offset));
    const got = dump(term);
    const diffs = differences(expected, got);
    if (diffs.length) {
      checkpointBad++;
      if (examples.length < 4) examples.push(`checkpoint @ ${c.offset}: ${diffs.join('; ')}`);
    }
    if (textDifferences(data.alacritty, got).length) alacrittyBad++;
    term.dispose();
  }
  reference.dispose();

  console.log(`\n=== ${data.name}: ${data.cases.length} checkpoints`);
  console.log(`  vs xterm.js fed the raw stream: ${checkpointBad} differ`);
  console.log(`  vs alacritty's text:            ${alacrittyBad} differ`);
  console.log(
    `  (xterm.js vs alacritty on the raw stream alone: ${baseline.length ? baseline.join('; ') : 'same text'})`,
  );
  for (const e of examples) console.log(`  ${e}`);
  summary.push(`${data.name.padEnd(10)} ${checkpointBad}/${data.cases.length} differ from xterm raw, ${alacrittyBad} from alacritty text`);
}
console.log('\n=== summary');
for (const line of summary) console.log(line);
