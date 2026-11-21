import { describe, expect, test } from '@jest/globals';

import { capitalised, sentence } from './text';

describe('sentence', () => {
  test('capitalises a reason and ends it', () => {
    expect(sentence("the repository's own checkout can't be pinned")).toBe("The repository's own checkout can't be pinned.");
  });

  test('leaves a reason that already ends as a sentence does', () => {
    expect(sentence('it has uncommitted changes.')).toBe('It has uncommitted changes.');
    expect(sentence('not now!')).toBe('Not now!');
    expect(sentence('ready?')).toBe('Ready?');
    expect(sentence('still starting…')).toBe('Still starting…');
  });
});

describe('capitalised', () => {
  test('capitalises and adds nothing', () => {
    expect(capitalised('running the tests')).toBe('Running the tests');
    expect(capitalised('')).toBe('');
  });
});
