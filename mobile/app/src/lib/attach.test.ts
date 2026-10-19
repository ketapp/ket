import { describe, expect, test } from '@jest/globals';

import { bytesLabel, fileName, splitFiles } from './attach';

describe('splitFiles', () => {
  test('splits off the files the desktop listed under the words', () => {
    expect(splitFiles('Look at the log\n\nAttached files:\n- /up/1/console.txt\n- /up/1/b.pdf')).toEqual({
      words: 'Look at the log',
      files: ['/up/1/console.txt', '/up/1/b.pdf'],
    });
  });

  test('takes a list with no words before it', () => {
    expect(splitFiles('Attached files:\n- /up/1/console.txt')).toEqual({ words: '', files: ['/up/1/console.txt'] });
  });

  test('leaves words that only mention the heading alone', () => {
    const text = 'What do you make of Attached files:\nthis line';
    expect(splitFiles(text)).toEqual({ words: text, files: [] });
    expect(splitFiles('plain')).toEqual({ words: 'plain', files: [] });
  });
});

test('bytesLabel and fileName read the way a tile shows them', () => {
  expect(bytesLabel(512)).toBe('512 B');
  expect(bytesLabel(14 * 1024)).toBe('14 KB');
  expect(bytesLabel(2.1 * 1024 * 1024)).toBe('2.1 MB');
  expect(fileName('/up/1/console.txt')).toBe('console.txt');
});
