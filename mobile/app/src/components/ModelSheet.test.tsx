import { describe, expect, jest, test } from '@jest/globals';
import { fireEvent, render } from '@testing-library/react-native';
import type { AgentModes, AgentSettings } from '@ket/remote';
import { ModelSheet } from './ModelSheet';

jest.mock('react-native-safe-area-context', () => ({ useSafeAreaInsets: () => ({ top: 0, bottom: 0 }) }));

const choice = (id: string, label: string) => ({ id, label }) as AgentSettings['models'][number];

const claude = {
  model: 'opus',
  effort: 'high',
  models: [choice('fable', 'Fable'), choice('opus', 'Opus'), choice('sonnet', 'Sonnet'), choice('haiku', 'Haiku')],
  efforts: [choice('low', 'Low'), choice('medium', 'Medium'), choice('high', 'High'), choice('xhigh', 'Extra high')],
  savesDefault: true,
} as AgentSettings;

const modes = {
  current: 'default',
  choices: [
    { id: 'default', label: 'Default', detail: '' },
    { id: 'plan', label: 'Plan', detail: '' },
  ],
} as AgentModes;

const sheet = (fields: Partial<Parameters<typeof ModelSheet>[0]> = {}) => {
  const props = {
    open: true,
    onClose: jest.fn(),
    agent: 'Claude',
    modes,
    settings: claude,
    pending: '',
    said: null,
    busy: false,
    onMode: jest.fn(),
    onModel: jest.fn(),
    onEffort: jest.fn(),
    ...fields,
  };
  return { props, view: render(<ModelSheet {...props} />) };
};

describe('ModelSheet', () => {
  test('shows mode, model and effort, with the ones in use marked', async () => {
    const { view } = sheet();
    const shown = await view;
    expect(shown.getByText('Mode')).toBeTruthy();
    expect(shown.getByText('Model')).toBeTruthy();
    expect(shown.getByText('Effort')).toBeTruthy();
    expect(shown.getByLabelText('Default').props.accessibilityState).toMatchObject({ checked: true });
    expect(shown.getByLabelText('High').props.accessibilityState).toMatchObject({ checked: true });
    expect(shown.getByLabelText('Low').props.accessibilityState).toMatchObject({ checked: false });
    expect(shown.getByText('Claude also keeps your pick as its default for new sessions.')).toBeTruthy();
    await shown.unmount();
  });

  test('hands each pick to its handler, and never the one already in use', async () => {
    const { props, view } = sheet();
    const shown = await view;
    await fireEvent.press(shown.getByText('Sonnet'));
    expect(props.onModel).toHaveBeenCalledWith(expect.objectContaining({ id: 'sonnet', label: 'Sonnet' }));
    await fireEvent.press(shown.getByLabelText('Extra high'));
    expect(props.onEffort).toHaveBeenCalledWith(expect.objectContaining({ id: 'xhigh' }));
    await fireEvent.press(shown.getByLabelText('Plan'));
    expect(props.onMode).toHaveBeenCalledWith(expect.objectContaining({ id: 'plan' }));
    await fireEvent.press(shown.getByText('Opus'));
    await fireEvent.press(shown.getByLabelText('High'));
    expect(props.onModel).toHaveBeenCalledTimes(1);
    expect(props.onEffort).toHaveBeenCalledTimes(1);
    await shown.unmount();
  });

  test('holds every pick while the agent works, and says why', async () => {
    const { props, view } = sheet({ busy: true });
    const shown = await view;
    expect(shown.getByText("Claude is working. These can change once it's done.")).toBeTruthy();
    await fireEvent.press(shown.getByText('Sonnet'));
    await fireEvent.press(shown.getByLabelText('Low'));
    await fireEvent.press(shown.getByLabelText('Plan'));
    expect(props.onModel).not.toHaveBeenCalled();
    expect(props.onEffort).not.toHaveBeenCalled();
    expect(props.onMode).not.toHaveBeenCalled();
    await shown.unmount();
  });

  test('marks a pick going through and holds the rest until it lands', async () => {
    const { props, view } = sheet({ pending: 'Sonnet' });
    const shown = await view;
    expect(shown.getByText('Sonnet…')).toBeTruthy();
    expect(shown.getByText('Switching to Sonnet…')).toBeTruthy();
    // The tick moves to the pick from the tap, not once it lands.
    expect(shown.getByLabelText('Sonnet…').props.accessibilityState).toMatchObject({ checked: true });
    expect(shown.getByLabelText('Opus').props.accessibilityState).toMatchObject({ checked: false });
    await fireEvent.press(shown.getByText('Haiku'));
    expect(props.onModel).not.toHaveBeenCalled();
    await shown.unmount();
  });

  test('raises an effort going through in place of the one in use', async () => {
    const { props, view } = sheet({ pending: 'Low' });
    const shown = await view;
    expect(shown.getByLabelText('Low').props.accessibilityState).toMatchObject({ checked: true });
    expect(shown.getByLabelText('High').props.accessibilityState).toMatchObject({ checked: false });
    // A model's pick leaves the effort marked where it is.
    await shown.rerender(<ModelSheet {...props} pending="Sonnet" />);
    expect(shown.getByLabelText('High').props.accessibilityState).toMatchObject({ checked: true });
    await shown.unmount();
  });

  test('says how the last pick went at its foot', async () => {
    const { props, view } = sheet({ said: { text: 'Switched to Sonnet.', failed: false } });
    const shown = await view;
    expect(shown.getByText('Switched to Sonnet.')).toBeTruthy();
    expect(shown.queryByText('Claude also keeps your pick as its default for new sessions.')).toBeNull();
    const why = 'Not switched: Claude is busy — change it once this turn is done';
    await shown.rerender(<ModelSheet {...props} said={{ text: why, failed: true }} />);
    expect(shown.getByText(why).props.style).toMatchObject({ color: expect.any(String) });
    await shown.unmount();
  });

  test("says a Codex pick is for the session only, and leaves mode out when there's none", async () => {
    const codex = {
      ...claude,
      model: 'gpt-6.1-sol',
      models: [choice('gpt-6.1-sol', 'gpt-6.1-sol'), choice('gpt-6-luna', 'gpt-6-luna')],
      savesDefault: false,
    } as AgentSettings;
    const { view } = sheet({ agent: 'Codex', settings: codex, modes: undefined });
    const shown = await view;
    expect(shown.getByText('For this session only.')).toBeTruthy();
    expect(shown.queryByText('Mode')).toBeNull();
    expect(shown.getByText('gpt-6-luna')).toBeTruthy();
    await shown.unmount();
  });

  test('closes from the dimmed backdrop, its close button, and the screen reader\'s escape gesture', async () => {
    const { props, view } = sheet();
    const shown = await view;
    // Behind the modal sheet, out of a screen reader's reach.
    expect(shown.queryByLabelText('Dismiss')).toBeNull();
    await fireEvent.press(shown.getByLabelText('Dismiss', { includeHiddenElements: true }));
    expect(props.onClose).toHaveBeenCalledTimes(1);
    await fireEvent(shown.getByLabelText('Claude settings'), 'accessibilityEscape');
    expect(props.onClose).toHaveBeenCalledTimes(2);
    await fireEvent.press(shown.getByLabelText('Close'));
    expect(props.onClose).toHaveBeenCalledTimes(3);
    await shown.unmount();
  });
});
