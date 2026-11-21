import { describe, expect, jest, test } from '@jest/globals';
import { act, fireEvent, render, waitFor } from '@testing-library/react-native';
import { ScrollView, StyleSheet, Text, View } from 'react-native';
import { getAnimatedStyle } from 'react-native-reanimated';
import type { Risk } from '@ket/remote';
import { CommandMenu } from './Commands';
import { PlusButton, SendButton } from './Composer';
import { FloatMenu } from './FloatMenu';
import * as Icons from './icons';
import { RiskBadge } from './Risk';
import { GLYPHS } from '../lib/glyphs';
import { READABLE_WIDTH, SplitContext } from '../lib/layout';
import {
  Badge,
  Banner,
  Bar,
  Button,
  Caption,
  Card,
  Dot,
  Figure,
  Heading,
  IconButton,
  Meter,
  ProjectHeading,
  Prose,
  Readout,
  Row,
  Screen,
  Segmented,
  TabBar,
  TabStrip,
  Tag,
  Toggle,
  Value,
} from './ui';

const mockRouter = { canGoBack: jest.fn(() => true), back: jest.fn(), push: jest.fn(), replace: jest.fn(), canDismiss: jest.fn(() => false), dismissAll: jest.fn() };

jest.mock('expo-router', () => ({
  router: {
    canGoBack: () => mockRouter.canGoBack(),
    back: () => mockRouter.back(),
    push: (path: string) => mockRouter.push(path),
    replace: (path: string) => mockRouter.replace(path),
    canDismiss: () => mockRouter.canDismiss(),
    dismissAll: () => mockRouter.dismissAll(),
  },
}));
jest.mock('react-native-safe-area-context', () => ({ useSafeAreaInsets: () => ({ top: 12, bottom: 24 }) }));

// TypeScript resolves reanimated's web build, where this is a no-op; jest runs
// the native one, which reads the style a component's animation has reached.
const animatedStyle = getAnimatedStyle as unknown as (component: { props: object }) => { opacity?: unknown };

describe('project badge', () => {
  // The desktop sends a project's icon from 1.27: a glyph it names is drawn,
  // text typed there is shown, and anything else falls back to the initial.
  test('draws the glyph the desktop gave the project', async () => {
    const view = await render(<Badge name="clinic-api" color="#14b8a6" icon="glyph:stethoscope" />);
    expect(view.getByTestId('badge-glyph')).toBeTruthy();
    expect(view.queryByText('D')).toBeNull();
  });

  test('shows a typed icon, and the initial for none or a glyph this phone lacks', async () => {
    const typed = await render(<Badge name="ket" color="#ef4444" icon="KT" />);
    expect(typed.getByText('KT')).toBeTruthy();
    const none = await render(<Badge name="ket" color="#ef4444" icon="" />);
    expect(none.getByText('K')).toBeTruthy();
    const unknown = await render(<Badge name="ket" color="#ef4444" icon="glyph:unicorn" />);
    expect(unknown.getByText('K')).toBeTruthy();
    expect(unknown.queryByTestId('badge-glyph')).toBeNull();
  });

  test('a name the table inherits is not a glyph', async () => {
    for (const icon of ['glyph:constructor', 'glyph:toString', 'glyph:__proto__']) {
      const view = await render(<Badge name="ket" color="#ef4444" icon={icon} />);
      expect(view.getByText('K')).toBeTruthy();
      expect(view.queryByTestId('badge-glyph')).toBeNull();
    }
  });

  test('every glyph the desktop has is one the phone can draw', () => {
    // The table is generated from the desktop's files; a shape the phone
    // cannot draw would be dropped silently, so each kind is checked here.
    const kinds = new Set(['path', 'circle', 'ellipse', 'rect', 'line', 'polyline', 'polygon']);
    expect(Object.keys(GLYPHS).length).toBe(47);
    for (const [name, glyph] of Object.entries(GLYPHS)) {
      // Each carries the grid it was drawn on: they are not all one size.
      expect(/^0 0 \d+ \d+$/.test(glyph.viewBox) ? name : `${name}: ${glyph.viewBox}`).toBe(name);
      expect(glyph.parts.length).toBeGreaterThan(0);
      for (const part of glyph.parts) expect(kinds.has(part.kind) ? name : `${name}: ${part.kind}`).toBe(name);
    }
  });
});

describe('shared UI controls', () => {
  test('renders layout, content, states, and optional decorations', async () => {
    const pressed = jest.fn();
    const toggled = jest.fn();
    const view = await render(
      <Screen>
        <Bar back title="Title" detail="Detail" lead={<Text>Lead</Text>} right={<Text>Right</Text>} />
        <Card>
          <Heading count={2} color="#fff">Section</Heading>
          <ProjectHeading name="alpha" color="#101010" collapsed waiting onToggle={toggled} />
          <Row
            mark={<Dot color="#fff" />}
            title="Worktree"
            detail="Working"
            state={{ word: 'running', color: '#0f0' }}
            meta={<Tag mono>+2</Tag>}
            marker
            chevron
            small
            onPress={pressed}
          >
            <Text>Child</Text>
          </Row>
          <Figure label="Changed"><Value color="#0f0">4</Value></Figure>
          <Readout><Text>Readout</Text></Readout>
          <Prose color="#fff">Words</Prose>
          <Banner tone="#ff0000"><Text>Warning</Text></Banner>
          <Badge name=" beta" color="#ffffff" />
          <Badge name="" color="#000000" size={30} />
          <Meter fraction={0.5} />
          <Meter fraction={-1} />
          <Meter fraction={Number.NaN} />
        </Card>
        <IconButton label="Icon action" onPress={pressed} well><Text>I</Text></IconButton>
        <Toggle label="Phones" on={false} onChange={toggled} />
      </Screen>,
    );
    const { getByText, getByLabelText } = view;

    for (const text of ['Title', 'Detail', 'Lead', 'Right', 'Section', 'alpha', 'Worktree', 'running', 'Child', 'Readout']) {
      expect(getByText(text)).toBeTruthy();
    }
    await fireEvent.press(getByText('Worktree'));
    await fireEvent.press(getByText('alpha'));
    await fireEvent.press(getByLabelText('Icon action'));
    await fireEvent.press(getByLabelText('Phones'));
    expect(pressed).toHaveBeenCalledTimes(2);
    expect(toggled).toHaveBeenCalledWith(true);
    await view.unmount();
  });

  test('draws a status caption without the markdown marks', async () => {
    const view = await render(
      <View>
        <Caption text="Editing `src/main.rs` now" />
        <Row title="idle" detail="*Searching* files" markdown />
      </View>,
    );
    expect(view.getByText('Editing src/main.rs now')).toBeTruthy();
    expect(view.queryByText(/`src\/main.rs`/)).toBeNull();
    expect(view.getByText('Searching files')).toBeTruthy();
    expect(view.queryByText('*Searching*')).toBeNull();
    await view.unmount();
  });

  test('handles navigation, segments, button kinds, and tabs', async () => {
    const change = jest.fn();
    const press = jest.fn();
    const view = await render(
      <View>
        <Bar back title="Back test" />
        <Segmented
          segments={[{ key: 'all', label: 'All', count: 2 }, { key: 'idle', label: 'Idle', icon: <Text>i</Text> }]}
          value="all"
          onChange={change}
        />
        {(['primary', 'brand', 'secondary', 'ghost', 'danger'] as const).map((kind) => (
          <Button key={kind} title={kind} kind={kind} onPress={press} disabled={kind === 'danger'} />
        ))}
        <TabBar active="needs" needs={3} />
      </View>,
    );
    const { getByLabelText, getByText } = view;

    await fireEvent.press(getByLabelText('Back'));
    expect(mockRouter.back).toHaveBeenCalled();
    mockRouter.canGoBack.mockReturnValueOnce(false);
    await fireEvent.press(getByLabelText('Back'));
    expect(mockRouter.replace).toHaveBeenCalledWith('/');
    await fireEvent.press(getByText('Idle'));
    expect(change).toHaveBeenCalledWith('idle');
    await fireEvent.press(getByText('primary'));
    expect(press).toHaveBeenCalled();
    expect(getByLabelText('Needs you, 3')).toBeTruthy();
    await fireEvent.press(getByLabelText('Usage'));
    expect(mockRouter.push).toHaveBeenCalledWith('/usage');
    await fireEvent.press(getByLabelText('Worktrees'));
    expect(mockRouter.replace).toHaveBeenCalledWith('/');
    await view.unmount();

    const usage = await render(<TabBar active="usage" needs={0} />);
    await fireEvent.press(usage.getByLabelText('Worktrees'));
    expect(mockRouter.back).toHaveBeenCalled();
    await usage.unmount();
  });

  test('a tab strip scrolls to the chosen tab, fades where there is more, and names a state', async () => {
    const change = jest.fn();
    const tabs = [
      { key: 'a', label: 'Claude', icon: <Text>c</Text> },
      { key: 'b', label: 'npm run start' },
      { key: 'c', label: 'Review changes', icon: <Text>x</Text>, state: { word: 'running', color: '#4cd08a' } },
    ];
    // React Native's ScrollView mock keeps one scrollTo for every instance.
    const scrollTo = jest.mocked(ScrollView.prototype.scrollTo);
    scrollTo.mockClear();
    const view = await render(<TabStrip label="Terminals in main" tabs={tabs} value="c" onChange={change} />);
    const [strip] = view.container.queryAll((x) => x.type === 'RCTScrollView');
    // The fades are the strip's only views that let touches through.
    const fades = () => view.container.queryAll((x) => x.props.pointerEvents === 'none').length;

    expect(view.getByLabelText('Terminals in main')).toBeTruthy();
    expect(view.getByLabelText('Review changes, running').props.accessibilityState).toEqual({ selected: true });
    expect(view.getByLabelText('Claude').props.accessibilityState).toEqual({ selected: false });

    // Three tabs 120 wide in a strip 200 wide: the chosen one, last, is
    // scrolled to once it and the strip are laid out, at once.
    await fireEvent(view.getByLabelText('Claude'), 'layout', { nativeEvent: { layout: { x: 3, width: 120 } } });
    await fireEvent(view.getByLabelText('npm run start'), 'layout', { nativeEvent: { layout: { x: 125, width: 120 } } });
    await fireEvent(view.getByLabelText('Review changes, running'), 'layout', { nativeEvent: { layout: { x: 247, width: 120 } } });
    await fireEvent(strip, 'contentSizeChange', 370, 40);
    await fireEvent(strip, 'layout', { nativeEvent: { layout: { width: 200 } } });
    expect(scrollTo).toHaveBeenCalledWith({ x: 195, animated: false });
    expect(fades()).toBe(1);

    // Scrolled to the middle: a fade at each end. Once revealed, the same
    // choice is not scrolled to again.
    await fireEvent.scroll(strip, { nativeEvent: { contentOffset: { x: 80 } } });
    expect(fades()).toBe(2);
    await fireEvent(strip, 'layout', { nativeEvent: { layout: { width: 200 } } });
    expect(scrollTo).toHaveBeenCalledTimes(1);

    // A new choice glides back to the first tab.
    await view.rerender(<TabStrip label="Terminals in main" tabs={tabs} value="a" onChange={change} />);
    expect(scrollTo).toHaveBeenLastCalledWith({ x: 0, animated: true });
    await fireEvent.scroll(strip, { nativeEvent: { contentOffset: { x: 0 } } });
    expect(fades()).toBe(1);

    await fireEvent.press(view.getByText('npm run start'));
    expect(change).toHaveBeenCalledWith('b');
    await view.unmount();
  });

  test('beside the sidebar, leaves out its tab bar and a Back with nowhere to go', async () => {
    mockRouter.canGoBack.mockReturnValue(false);
    const view = await render(
      <SplitContext.Provider value>
        <Screen>
          <Bar back title="First" />
          <TabBar active="needs" needs={0} />
        </Screen>
      </SplitContext.Provider>,
    );
    expect(view.queryByLabelText('Back')).toBeNull();
    expect(view.queryByLabelText('Needs you')).toBeNull();
    await view.unmount();

    mockRouter.canGoBack.mockReturnValue(true);
    const deeper = await render(
      <SplitContext.Provider value>
        <Bar back title="Deeper" />
        <TabBar active="needs" needs={0} sidebar />
      </SplitContext.Provider>,
    );
    expect(deeper.getByLabelText('Back')).toBeTruthy();
    expect(deeper.getByLabelText('Needs you')).toBeTruthy();
    await deeper.unmount();
  });

  test('draws a terminal across the whole width and everything else in a column', async () => {
    const view = await render(
      <View>
        <Screen>
          <Text>Column</Text>
        </Screen>
        <Screen full>
          <Text>Full</Text>
        </Screen>
      </View>,
    );
    // The nearest narrower box around the text, if any.
    const widthOf = (text: string) => {
      for (let node = view.getByText(text).parent; node; node = node.parent) {
        const maxWidth = StyleSheet.flatten(node.props.style)?.maxWidth;
        if (maxWidth !== undefined) return maxWidth;
      }
      return undefined;
    };
    expect(widthOf('Column')).toBe(READABLE_WIDTH);
    expect(widthOf('Full')).toBeUndefined();
    await view.unmount();
  });
});

describe('small components', () => {
  test('Send is pressable only with something to send, and is Stop while the agent works', async () => {
    const press = jest.fn();
    const view = await render(<SendButton ready={false} stop={false} stopping={false} launches={0} onPress={press} />);
    await fireEvent.press(view.getByLabelText('Send'));
    expect(press).not.toHaveBeenCalled();
    await view.rerender(<SendButton ready stop={false} stopping={false} launches={0} onPress={press} />);
    await fireEvent.press(view.getByLabelText('Send'));
    expect(press).toHaveBeenCalledTimes(1);
    await view.rerender(<SendButton ready={false} stop stopping={false} launches={1} onPress={press} />);
    expect(view.queryByLabelText('Send')).toBeNull();
    await fireEvent.press(view.getByLabelText('Stop'));
    expect(press).toHaveBeenCalledTimes(2);
    // Stop pressed and not yet taken: not again.
    await view.rerender(<SendButton ready={false} stop stopping launches={1} onPress={press} />);
    await fireEvent.press(view.getByLabelText('Stop'));
    expect(press).toHaveBeenCalledTimes(2);
    await view.unmount();
  });

  test('+ says what it does either way, and a closed menu in place is out of the tree', async () => {
    const press = jest.fn();
    const photos = [{ items: [{ key: 'photos', label: 'Photos', onPress: jest.fn() }] }];
    const menu = (open: boolean) => (
      <>
        <PlusButton open={open} onPress={press} />
        <FloatMenu inline={{ left: 0 }} open={open} onClose={jest.fn()} label="Attach" sections={photos} />
      </>
    );
    const view = await render(menu(false));
    expect(view.queryByText('Photos')).toBeNull();
    await fireEvent.press(view.getByLabelText('Attach a photo or file'));
    expect(press).toHaveBeenCalled();
    await view.rerender(menu(true));
    expect(view.getByLabelText('Close the attach menu')).toBeTruthy();
    expect(view.getByText('Photos')).toBeTruthy();
    await view.rerender(menu(false));
    // Gone once its close has played.
    await waitFor(() => expect(view.queryByText('Photos')).toBeNull());
    await view.unmount();
  });

  test('a menu in place grows in once its card is laid out, not on the frame it goes in', async () => {
    jest.useFakeTimers();
    try {
      const photos = [{ items: [{ key: 'photos', label: 'Photos', onPress: jest.fn() }] }];
      const view = await render(<FloatMenu inline={{ left: 0 }} open onClose={jest.fn()} label="Attach" sections={photos} />);
      const card = view.getByLabelText('Attach');
      await act(async () => {
        await jest.advanceTimersByTimeAsync(600);
      });
      expect(animatedStyle(card).opacity).toBe(0);
      await fireEvent(card, 'layout', { nativeEvent: { layout: { x: 0, y: 0, width: 280, height: 60 } } });
      await act(async () => {
        await jest.advanceTimersByTimeAsync(600);
      });
      expect(animatedStyle(card).opacity).toBe(1);
      await view.unmount();
    } finally {
      jest.useRealTimers();
    }
  });

  test('selects slash commands', async () => {
    const pick = jest.fn();
    const commands = [
      { name: '/model', about: 'Choose model', picker: true },
      { name: '/clear', about: 'Start over' },
    ];
    const view = await render(<CommandMenu commands={commands} onPick={pick} />);
    const { getByText } = view;
    expect(getByText('in terminal')).toBeTruthy();
    await fireEvent.press(getByText('/model'));
    expect(pick).toHaveBeenCalledWith(commands[0]);
    await view.unmount();
  });

  test('shows only recognized risk levels', async () => {
    const risk = { level: 'high', confidence: 0.914 } as Risk;
    const shown = await render(<RiskBadge risk={risk} />);
    expect(shown.getByText('High risk · 91%')).toBeTruthy();
    expect(shown.getByLabelText('High risk, 91 percent sure. Beta.')).toBeTruthy();
    await shown.unmount();
    const absent = await render(<RiskBadge />);
    expect(absent.toJSON()).toBeNull();
    await absent.unmount();
    const unknown = await render(<RiskBadge risk={{ level: 'unknown' } as Risk} />);
    expect(unknown.toJSON()).toBeNull();
    await unknown.unmount();
  });

  test('renders every icon and agent-mark branch', async () => {
    const iconComponents = [
      Icons.Plus,
      Icons.Back,
      Icons.Chevron,
      Icons.Desktop,
      Icons.Sliders,
      Icons.Search,
      Icons.TerminalGlyph,
      Icons.Branch,
      Icons.Clipboard,
      Icons.Send,
      Icons.Tree,
      Icons.Gauge,
      Icons.Inbox,
      Icons.Camera,
      Icons.Hand,
      Icons.Keyboard,
      Icons.Expand,
      Icons.Collapse,
      Icons.FaceScan,
      Icons.Fingerprint,
      Icons.Erase,
    ];
    const view = await render(
      <View>
        <Icons.BrandMark />
        {['codex', 'grok', 'claude', 'gemini', 'opencode'].map((agent) => <Icons.AgentMark key={agent} agent={agent} />)}
        {iconComponents.map((Icon, index) => <Icon key={index} size={20} />)}
      </View>,
    );
    expect(view.toJSON()).toBeTruthy();
    await view.unmount();
  });
});

describe('session colours', () => {
  type Rendered = { props: { style?: unknown }; children: (Rendered | string)[] | null };
  // Every rendered node filled with `color`.
  const filled = (tree: unknown, color: string): Rendered[] => {
    if (!tree || typeof tree === 'string') return [];
    if (Array.isArray(tree)) return tree.flatMap((node) => filled(node, color));
    const node = tree as Rendered;
    const own = StyleSheet.flatten(node.props.style as never)?.backgroundColor === color ? [node] : [];
    return [...own, ...filled(node.children, color)];
  };

  test('a mark given its session\'s colour sits on a tile of it', async () => {
    const view = await render(<Icons.AgentMark agent="claude" size={16} session="red" />);
    const tiles = filled(view.toJSON(), '#ef4444');
    expect(tiles.length).toBeGreaterThan(0);
    expect(StyleSheet.flatten(tiles[0].props.style as never)).toMatchObject({ width: 22, height: 22 });
    await view.unmount();
  });

  test('a mark with no colour, or one this build does not know, sits on none', async () => {
    for (const session of [undefined, '', 'magenta']) {
      const view = await render(<Icons.AgentMark agent="claude" size={16} session={session} />);
      expect(filled(view.toJSON(), '#ef4444')).toHaveLength(0);
      await view.unmount();
    }
  });

  test('a row\'s title takes its colour, unless the row is faded', async () => {
    const view = await render(
      <Card>
        <Row title="coloured" titleColor="#ef4444" />
        <Row title="faded" titleColor="#ef4444" faded />
      </Card>,
    );
    expect(StyleSheet.flatten(view.getByText('coloured').props.style).color).toBe('#ef4444');
    expect(StyleSheet.flatten(view.getByText('faded').props.style).color).not.toBe('#ef4444');
    await view.unmount();
  });
});
