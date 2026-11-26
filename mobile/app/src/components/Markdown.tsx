// An agent's words read as markdown — see lib/markdown.ts — and drawn as the
// chat draws them: fenced code as a block of its own, tables sideways, the
// rest as prose with its headings, lists, quotes and emphasis drawn rather
// than shown as marks, and links that open.

import { Fragment, type ReactNode } from 'react';
import { Linking, Pressable, ScrollView, StyleSheet, Text, TouchableOpacity, View } from 'react-native';

import { Check } from './icons';
import { type Block, type Run, markdown } from '../lib/markdown';
import { font, size, t } from '../lib/theme';
import { InlineCode } from './ui/rows';
import { PRESSED, surface } from './ui/surface';

/** An agent's words, read as markdown — see lib/markdown.ts: fenced code as
 * a block of its own, tables sideways, the rest as prose with its headings,
 * lists, quotes and emphasis drawn rather than shown as marks. */
export function Words({ text, onCopy }: { text: string; onCopy: (text: string) => void }) {
  // Prose in a run of its own, so its lines keep their spacing; code and
  // tables between.
  const out: ReactNode[] = [];
  let prose: Block[] = [];
  const flush = () => {
    if (prose.length === 0) return;
    out.push(
      <View key={out.length} style={styles.prose}>
        {prose.map((block, n) => (
          <Line key={n} block={block} />
        ))}
      </View>,
    );
    prose = [];
  };
  for (const block of markdown(text)) {
    if (block.kind === 'code') {
      flush();
      out.push(<Code key={out.length} code={block.text} onHold={() => onCopy(text)} onCopy={onCopy} />);
    } else if (block.kind === 'table') {
      flush();
      out.push(<Table key={out.length} table={block} onHold={() => onCopy(text)} />);
    } else {
      prose.push(block);
    }
  }
  flush();
  return <>{out}</>;
}

/** A fenced block of code, sideways to scroll, with its own Copy. */
function Code({ code, onHold, onCopy }: { code: string; onHold: () => void; onCopy: (text: string) => void }) {
  return (
    <View style={styles.codeBlock}>
      <ScrollView horizontal>
        {/* The hold that copies the turn, inside the scroller — see `Table`. */}
        <Pressable accessibilityHint="Hold to copy" delayLongPress={350} onLongPress={onHold} style={styles.codeBlockInner}>
          <Text style={styles.codeText}>{code}</Text>
        </Pressable>
      </ScrollView>
      <TouchableOpacity
        accessibilityRole="button"
        accessibilityLabel="Copy code"
        hitSlop={6}
        onPress={() => onCopy(code)}
        activeOpacity={PRESSED}
        style={styles.codeCopy}
      >
        <Text style={styles.codeCopyText}>Copy</Text>
      </TouchableOpacity>
    </View>
  );
}

/** A heading's size, by level: the body's for the fourth and below. */
const HEADING = [
  { fontSize: 18, lineHeight: 25 },
  { fontSize: 16.5, lineHeight: 23 },
  { fontSize: 15, lineHeight: 22 },
];

/** One block of prose, drawn; dim inside a quote. A list item hangs its
 * wrapped lines under its words, not its marker. */
function Line({ block, dim }: { block: Block; dim?: boolean }) {
  const color = dim ? { color: t.dim } : null;
  switch (block.kind) {
    case 'heading':
      return (
        <Text style={[styles.agentText, styles.strong, HEADING[Math.min(block.level, 3) - 1], block.level <= 2 && styles.headingGap, color]}>
          <Runs runs={block.runs} />
        </Text>
      );
    case 'item':
      return (
        <View style={[styles.item, { paddingLeft: Math.min(block.depth, 4) * 16 }]}>
          <Text style={[styles.agentText, styles.marker]}>{block.marker}</Text>
          <Text style={[styles.agentText, styles.itemText, color]}>
            <Runs runs={block.runs} />
          </Text>
        </View>
      );
    case 'task':
      return (
        <View style={[styles.item, { paddingLeft: Math.min(block.depth, 4) * 16 }]}>
          <View style={[styles.box, block.done && styles.boxDone]}>
            {block.done ? <Check size={11} color={t.onAccent} weight={3} /> : null}
          </View>
          <Text style={[styles.agentText, styles.itemText, (block.done || dim) && { color: t.dim }]}>
            <Runs runs={block.runs} />
          </Text>
        </View>
      );
    case 'quote':
      return (
        <View style={[styles.quote, styles.prose]}>
          {block.blocks.map((inner, n) =>
            inner.kind === 'code' ? (
              <Text key={n} style={[styles.codeText, { color: t.dim }]}>
                {inner.text}
              </Text>
            ) : (
              <Line key={n} block={inner} dim />
            ),
          )}
        </View>
      );
    case 'rule':
      return <View style={styles.rule} />;
    case 'para':
      return (
        <Text style={[styles.agentText, color]}>
          <Runs runs={block.runs} />
        </Text>
      );
    default:
      // A table in a quote, as its cells' words.
      return block.kind === 'table' ? (
        <Text style={[styles.agentText, color]}>
          {[block.head, ...block.rows].map((row) => row.map(plain).join(' · ')).join('\n')}
        </Text>
      ) : null;
  }
}

/** Runs of text as markdown marked them: `code`, **strong**, *emphasis*,
 * ~~struck~~ and links, nested as written. On one line — in a table's cell —
 * a break is a space. */
function Runs({ runs, flat }: { runs: Run[]; flat?: boolean }) {
  return (
    <>
      {runs.map((piece, at) => {
        switch (piece.kind) {
          case 'text':
            return <Fragment key={at}>{flat ? piece.text.replace(/\n/g, ' ') : piece.text}</Fragment>;
          case 'code':
            return (
              <InlineCode key={at} style={styles.inlineCode}>
                {piece.text}
              </InlineCode>
            );
          case 'strong':
          case 'em':
          case 'struck':
            return (
              <Text key={at} style={piece.kind === 'strong' ? styles.strong : piece.kind === 'em' ? styles.emphasis : styles.struck}>
                <Runs runs={piece.runs} flat={flat} />
              </Text>
            );
          case 'link':
            return (
              <Text key={at} style={styles.link} onPress={() => void Linking.openURL(piece.url)}>
                <Runs runs={piece.runs} flat={flat} />
              </Text>
            );
          case 'break':
            return <Fragment key={at}>{flat ? ' ' : '\n'}</Fragment>;
        }
      })}
    </>
  );
}

/** Runs as their words alone. */
export function plain(runs: Run[]): string {
  return runs.map((piece) => ('runs' in piece ? plain(piece.runs) : piece.kind === 'break' ? ' ' : piece.text)).join('');
}

/** A web address in a message of yours, `[words](https://…)` or bare. */
const LINK = /\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)|(https?:\/\/[^\s<>"'`]+)/g;

/** Your own words with their links made tappable, opened in the phone's
 * browser, and otherwise as you typed them. A bare address leaves the
 * punctuation that ends its sentence outside it. */
export function Linked({ text }: { text: string }) {
  const out: ReactNode[] = [];
  let from = 0;
  for (const match of text.matchAll(LINK)) {
    const at = match.index ?? 0;
    let shown = match[1] ?? match[3];
    let url = match[2] ?? match[3];
    let rest = '';
    if (match[3]) {
      const trimmed = url.replace(/[.,:;!?)\]*_~]+$/, '');
      rest = url.slice(trimmed.length);
      url = shown = trimmed;
    }
    if (at > from) out.push(text.slice(from, at));
    out.push(
      <Text key={at} style={styles.link} onPress={() => void Linking.openURL(url)}>
        {shown}
      </Text>,
    );
    if (rest) out.push(rest);
    from = at + match[0].length;
  }
  if (from < text.length) out.push(text.slice(from));
  return <>{out}</>;
}

/** A table, column by column so each column takes its widest cell, and
 * sideways to scroll when it is wider than the phone. A narrower one stretches
 * its columns to fill the frame. Cells stay on one line,
 * which keeps the rows of every column level with each other.
 *
 * The frame and its rounded clip are on a view around the scroller, not on the
 * scroller itself: `overflow: 'hidden'` on a ScrollView replaces the `scroll`
 * its layout depends on, and the columns are then squeezed to the phone's
 * width and cut off instead of running past it.
 *
 * The hold that copies the turn is here, inside the scroller, and not only on
 * the turn around it. On iOS a scroll view will not start a drag while a view
 * above it holds the touch, and the turn's Pressable takes every touch as it
 * lands: a sideways drag only got through when it beat that to the main
 * thread. A press inside the scroller takes the touch instead, and the scroll
 * view is free to cancel it and scroll. */
function Table({ table, onHold }: { table: Extract<Block, { kind: 'table' }>; onHold: () => void }) {
  return (
    <View style={styles.table}>
    <ScrollView horizontal contentContainerStyle={styles.tableFill}>
      <Pressable accessibilityHint="Hold to copy" delayLongPress={350} onLongPress={onHold} style={styles.tableInner}>
      {table.head.map((title, col) => {
        const align = { textAlign: table.align[col] ?? 'left' };
        return (
          <View key={col} style={[styles.tableColumn, col > 0 && styles.tableColumnRule]}>
            <View style={[styles.tableCell, styles.tableHead]}>
              <Text style={[styles.tableText, styles.strong, align]} numberOfLines={1}>
                <Runs runs={title} flat />
              </Text>
            </View>
            {table.rows.map((row, at) => (
              <View key={at} style={[styles.tableCell, styles.tableRowRule]}>
                <Text style={[styles.tableText, align]} numberOfLines={1}>
                  <Runs runs={row[col] ?? []} flat />
                </Text>
              </View>
            ))}
          </View>
        );
      })}
      </Pressable>
    </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  agentText: { fontFamily: font.prose, fontSize: size.body, lineHeight: 21, color: t.text },
  link: { color: t.attention, textDecorationLine: 'underline' },
  strong: { fontFamily: font.proseSemibold },
  emphasis: { fontStyle: 'italic' },
  struck: { textDecorationLine: 'line-through', color: t.dim },
  prose: { gap: 6 },
  headingGap: { marginTop: 4 },
  item: { flexDirection: 'row', gap: 7 },
  marker: { minWidth: 12, color: t.dim },
  itemText: { flex: 1, minWidth: 0 },
  box: {
    width: 15,
    height: 15,
    marginTop: 3,
    borderRadius: 4,
    borderWidth: 1.5,
    borderColor: t.dim,
    alignItems: 'center',
    justifyContent: 'center',
  },
  boxDone: { backgroundColor: t.accent, borderColor: t.accent },
  quote: { borderLeftWidth: 3, borderLeftColor: t.border, paddingLeft: 10 },
  rule: { height: 1, backgroundColor: t.rule, marginVertical: 6 },
  codeCopy: {
    position: 'absolute',
    top: 6,
    right: 6,
    height: 24,
    paddingHorizontal: 8,
    borderRadius: size.radiusSm,
    // On the code block's raised fill, so a step up from it.
    ...surface.chosen,
    justifyContent: 'center',
  },
  codeCopyText: { fontFamily: font.proseMedium, fontSize: 11.5, color: t.dim },
  inlineCode: { fontFamily: font.code, fontSize: size.label, color: t.text, backgroundColor: t.sunken },
  codeBlock: { ...surface.well, borderRadius: size.radius },
  table: { borderWidth: 1, borderColor: t.border, borderRadius: size.radius, overflow: 'hidden' },
  tableFill: { flexGrow: 1 },
  tableInner: { flexGrow: 1, flexDirection: 'row' },
  tableColumn: { flexGrow: 1, minWidth: 56 },
  tableColumnRule: { borderLeftWidth: 1, borderLeftColor: t.rule },
  tableCell: { height: 34, justifyContent: 'center', paddingHorizontal: 10 },
  tableHead: { backgroundColor: t.sunken },
  tableRowRule: { borderTopWidth: 1, borderTopColor: t.rule },
  tableText: { fontFamily: font.prose, fontSize: size.label, lineHeight: 18, color: t.text },
  codeBlockInner: { padding: 10 },
  codeText: { fontFamily: font.code, fontSize: 12.5, lineHeight: 18, color: t.text },
});
