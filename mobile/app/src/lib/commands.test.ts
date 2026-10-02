import { describe, expect, test } from '@jest/globals';

import { commandsFor, matching } from './commands';

describe('commandsFor', () => {
  test.each([
    ['claude', '/compact'],
    ['CODEX', '/diff'],
    ['Gemini', '/stats'],
    ['opencode', '/sessions'],
  ])('returns known commands for %s', (agent, command) => {
    expect(commandsFor(agent).map(({ name }) => name)).toContain(command);
  });

  test('returns no commands for shells and unknown agents', () => {
    expect(commandsFor('')).toEqual([]);
    expect(commandsFor('bash')).toEqual([]);
  });
});

describe('matching', () => {
  const commands = commandsFor('claude');

  test('matches slash-name prefixes without case sensitivity', () => {
    expect(matching(commands, '/CO').map(({ name }) => name)).toEqual(['/compact', '/context']);
  });

  test('shows every command for a bare slash', () => {
    expect(matching(commands, '/')).toEqual(commands);
  });

  test.each(['compact', '/compact now', ' /compact', '/compact/', 'hello'])('rejects non-prefix draft %p', (draft) => {
    expect(matching(commands, draft)).toEqual([]);
  });

  test('preserves metadata used by the composer', () => {
    expect(commands.find(({ name }) => name === '/review')?.args).toBe(true);
    expect(commands.find(({ name }) => name === '/model')?.picker).toBe(true);
  });
});
