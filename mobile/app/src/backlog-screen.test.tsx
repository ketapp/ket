import { afterEach, beforeEach, describe, expect, jest, test } from '@jest/globals';
import { fireEvent, render, waitFor } from '@testing-library/react-native';
import { useEffect as mockUseEffect } from 'react';
import { Alert, type AlertButton } from 'react-native';
import BacklogScreen from './app/backlog/[host]/[project]';

let mockConn: any;

jest.mock('expo-router', () => ({
  router: { back: jest.fn(), canGoBack: () => true },
  useFocusEffect: (callback: () => void) => mockUseEffect(callback, [callback]),
  useLocalSearchParams: () => ({ host: 'host', project: 'project' }),
}));
jest.mock('react-native-safe-area-context', () => ({ useSafeAreaInsets: () => ({ top: 0, bottom: 0 }) }));
jest.mock('@ket/remote', () => ({ BacklogPriority: { UNSPECIFIED: 0, LOW: 1, MEDIUM: 2, HIGH: 3, URGENT: 4 } }));
jest.mock('./lib/hosts', () => ({ useHost: () => mockConn }));

const note = (fields: object = {}) => ({
  id: 'n1',
  title: 'Fix the thing',
  body: 'Details',
  priority: 2,
  files: [],
  doneMs: 0n,
  branch: '',
  ...fields,
});

const connection = (notes: object[]) => {
  const backlog = { notes };
  return {
    status: 'connected',
    hostMinor: 10,
    host: { id: 'host', name: 'Studio Mac' },
    snapshot: { projects: [{ id: 'project', name: 'ket' }] },
    backlog: jest.fn(async () => backlog),
    editBacklog: jest.fn(async () => backlog),
    startNote: jest.fn(async () => null),
  };
};

/** The screen with `notes`, and the first open one opened. */
async function opened(notes: object[] = [note()]) {
  mockConn = connection(notes);
  const view = await render(<BacklogScreen />);
  await waitFor(() => expect(view.getByText('Fix the thing')).toBeTruthy());
  await fireEvent.press(view.getByText('Fix the thing'));
  return view;
}

beforeEach(() => {
  jest.clearAllMocks();
});

afterEach(() => {
  jest.useRealTimers();
});

describe('an open note', () => {
  test('keeps Delete apart from Mark done and Start', async () => {
    const view = await opened();
    expect(view.getByLabelText('Delete the note')).toBeTruthy();
    expect(view.getByText('Start')).toBeTruthy();
    expect(view.queryByText('Done')).toBeNull();
    await fireEvent.press(view.getByText('Mark done'));
    await waitFor(() => expect(mockConn.editBacklog).toHaveBeenCalledWith('project', 'n1', { action: 'done' }));
    await view.unmount();
  });

  test('asks before it is deleted', async () => {
    const alert = jest.spyOn(Alert, 'alert').mockImplementation(() => {});
    const view = await opened();
    await fireEvent.press(view.getByLabelText('Delete the note'));
    expect(alert).toHaveBeenCalledTimes(1);
    expect(alert.mock.calls[0][0]).toBe('Delete “Fix the thing”?');
    expect(mockConn.editBacklog).not.toHaveBeenCalled();
    const buttons = alert.mock.calls[0][2] as AlertButton[];
    buttons.find((button) => button.style === 'destructive')?.onPress?.();
    await waitFor(() => expect(mockConn.editBacklog).toHaveBeenCalledWith('project', 'n1', { action: 'remove' }));
    await view.unmount();
  });

  test('changed, offers Save where Start was, and Cancel puts it back', async () => {
    const view = await opened();
    await fireEvent.changeText(view.getByDisplayValue('Fix the thing'), 'Fix it');
    expect(view.getByText('Save')).toBeTruthy();
    for (const gone of ['Start', 'Mark done']) expect(view.queryByText(gone)).toBeNull();
    expect(view.queryByLabelText('Delete the note')).toBeNull();
    await fireEvent.press(view.getByText('Cancel'));
    expect(view.getByDisplayValue('Fix the thing')).toBeTruthy();
    expect(view.getByText('Start')).toBeTruthy();
    expect(view.queryByText('Save')).toBeNull();
    await view.unmount();
  });

  test('takes its priority from the track and saves it', async () => {
    const view = await opened();
    const track = ['Low', 'Medium', 'High', 'Urgent'].map((name) => view.getByText(name));
    expect(track.length).toBe(4);
    await fireEvent.press(view.getByText('High'));
    await fireEvent.press(view.getByText('Save'));
    await waitFor(() =>
      expect(mockConn.editBacklog).toHaveBeenCalledWith('project', 'n1', {
        save: { title: 'Fix the thing', body: 'Details', priority: 3 },
      }),
    );
    await view.unmount();
  });

  test('says it is starting in place of its actions', async () => {
    jest.useFakeTimers();
    const view = await opened();
    await fireEvent.press(view.getByText('Start'));
    await waitFor(() => expect(mockConn.startNote).toHaveBeenCalledWith('project', 'n1'));
    expect(view.getByText('Starting on Studio Mac…')).toBeTruthy();
    for (const gone of ['Start', 'Mark done']) expect(view.queryByText(gone)).toBeNull();
    expect(view.queryByLabelText('Delete the note')).toBeNull();
    await view.unmount();
  });
});

test('a done note, opened, keeps Delete apart from Reopen', async () => {
  mockConn = connection([note({ id: 'n2', title: 'Old work', doneMs: 1n, branch: 'old-work' })]);
  const view = await render(<BacklogScreen />);
  await waitFor(() => expect(view.getByText('Done')).toBeTruthy());
  await fireEvent.press(view.getByText('Done'));
  await fireEvent.press(view.getByText('Old work'));
  expect(view.getByLabelText('Delete the note')).toBeTruthy();
  await fireEvent.press(view.getByText('Reopen'));
  await waitFor(() => expect(mockConn.editBacklog).toHaveBeenCalledWith('project', 'n2', { action: 'reopen' }));
  await view.unmount();
});
