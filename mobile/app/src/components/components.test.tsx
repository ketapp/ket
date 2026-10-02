import { describe, expect, jest, test } from '@jest/globals';
import { fireEvent, render } from '@testing-library/react-native';
import { Text, View } from 'react-native';
import type { Risk } from '@ket/remote';
import { CommandMenu } from './Commands';
import * as Icons from './icons';
import { RiskBadge } from './Risk';
import {
  Badge,
  Banner,
  Bar,
  Button,
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
  Tag,
  Toggle,
  Value,
} from './ui';

const mockRouter = { canGoBack: jest.fn(() => true), back: jest.fn(), replace: jest.fn() };

jest.mock('expo-router', () => ({
  router: {
    canGoBack: () => mockRouter.canGoBack(),
    back: () => mockRouter.back(),
    replace: (path: string) => mockRouter.replace(path),
  },
}));
jest.mock('react-native-safe-area-context', () => ({ useSafeAreaInsets: () => ({ top: 12, bottom: 24 }) }));

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
    await fireEvent.press(getByLabelText('Worktrees'));
    expect(mockRouter.replace).toHaveBeenCalledWith('/');
    await view.unmount();
  });
});

describe('small components', () => {
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
        {['codex', 'grok', 'claude', 'gemini'].map((agent) => <Icons.AgentMark key={agent} agent={agent} />)}
        {iconComponents.map((Icon, index) => <Icon key={index} size={20} />)}
      </View>,
    );
    expect(view.toJSON()).toBeTruthy();
    await view.unmount();
  });
});
