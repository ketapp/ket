import { describe, expect, test } from '@jest/globals';

import { caption } from './markdown';

describe('caption', () => {
  test('leaves plain words alone', () => {
    expect(caption('Compile workspace')).toEqual([{ kind: 'text', text: 'Compile workspace' }]);
  });

  test('takes paired backticks as code, the way the Mac sidebar does', () => {
    expect(caption('Editing `src/main.rs` now')).toEqual([
      { kind: 'text', text: 'Editing ' },
      { kind: 'code', text: 'src/main.rs' },
      { kind: 'text', text: ' now' },
    ]);
    expect(caption('`alone`')).toEqual([{ kind: 'code', text: 'alone' }]);
  });

  test('keeps empty ticks and an unmatched tick in the string', () => {
    expect(caption('see `` later')).toEqual([{ kind: 'text', text: 'see `` later' }]);
    expect(caption('started `src/main.rs')).toEqual([{ kind: 'text', text: 'started `src/main.rs' }]);
  });

  test('takes **strong** and *emphasis*, and leaves unmatched asterisks', () => {
    expect(caption('Working on **the plan**')).toEqual([
      { kind: 'text', text: 'Working on ' },
      { kind: 'strong', runs: [{ kind: 'text', text: 'the plan' }] },
    ]);
    expect(caption('*Searching* files')).toEqual([
      { kind: 'em', runs: [{ kind: 'text', text: 'Searching' }] },
      { kind: 'text', text: ' files' },
    ]);
    expect(caption('a * b * c')).toEqual([{ kind: 'text', text: 'a * b * c' }]);
    expect(caption('started *Searching')).toEqual([{ kind: 'text', text: 'started *Searching' }]);
  });

  test('does not read asterisks inside code', () => {
    expect(caption('run `*glob*`')).toEqual([
      { kind: 'text', text: 'run ' },
      { kind: 'code', text: '*glob*' },
    ]);
  });

  test('nests emphasis inside strong', () => {
    expect(caption('**see *this* now**')).toEqual([
      {
        kind: 'strong',
        runs: [
          { kind: 'text', text: 'see ' },
          { kind: 'em', runs: [{ kind: 'text', text: 'this' }] },
          { kind: 'text', text: ' now' },
        ],
      },
    ]);
  });
});
