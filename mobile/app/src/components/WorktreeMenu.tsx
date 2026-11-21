// What holding a worktree offers, as the desktop's worktree menu has it: on
// Worktrees, held on its row, and on the worktree's own screen, from its bar.

import { type WorktreeSummary } from '@ket/remote';
import { router } from 'expo-router';
import { type ComponentType, createElement } from 'react';
import { Alert } from 'react-native';

import { type MenuItem, type MenuSection } from './FloatMenu';
import { Check, CheckCircle, Comment, Merge, Pencil, Pin, PinOff, PullRequestGlyph, Refresh, Trash } from './icons';
import { Dot } from './ui/marks';
import { agentTerminal, isUnread } from '../lib/activity';
import type { ConnectionState, HostConnection } from '../lib/connection';
import { confirmIdentity } from '../lib/lock';
import { askMerge, prActions } from '../lib/pr-actions';
import { t } from '../lib/theme';
import { sentence } from '../lib/text';

/** What holding a worktree offers: what the desktop's worktree menu does
 * that the phone can ask for too. Rename, Pin and Mark as unread first; then
 * its pull request's actions — or, with none open, pushing it and opening
 * one; then Delete.
 * The repository's own checkout is neither pinned nor deleted, and has no
 * pull request of its own. Empty when there is nothing to offer. The
 * worktree's own screen offers the same from its bar. */
export function worktreeMenu(
  conn: HostConnection,
  /** What `conn` is as the screen draws: the menu is built then. */
  hostState: ConnectionState,
  worktree: WorktreeSummary,
  open: (href: string) => void,
  /** Takes a worktree out of the list while it is deleted, or puts it back. */
  going: (id: string, away: boolean) => void,
): MenuSection[] {
  const pr = worktree.pullRequest;
  const host = hostState.host.name || 'your desktop';
  const name = worktree.name || worktree.branch || 'this worktree';
  // A repository's own checkout has no base: it is what the others branch
  // from, and what a pull request would merge into.
  const primary = worktree.base === '';
  // An element, not a call: an icon is a component, and one that uses a hook
  // may only run while React renders it, not while a menu is being built.
  const icon = (glyph: ComponentType<{ size?: number; color?: string }>, color: string = t.dim) =>
    createElement(glyph, { size: 17, color });
  const failed = (title: string) => (why: string | null) => {
    if (why) Alert.alert(title, sentence(why));
  };
  const reason = (e: unknown) => (e instanceof Error ? e.message : String(e));
  const sections: MenuSection[] = [];

  // The row's own dot back, until it is next opened, here or on the desktop
  // (1.35). Any worktree, the repository's own checkout too; not one that has
  // the dot already.
  const mark: MenuItem[] =
    hostState.hostMinor >= 35 && !isUnread(worktree)
      ? [
          {
            key: 'unread',
            label: 'Mark as unread',
            icon: createElement(Dot, { color: t.accent, size: 7 }),
            onPress: () => conn.markUnread(worktree.id, true),
          },
        ]
      : [];

  // Pin and Delete came with 1.20, Rename with 1.33.
  const manages = !primary && hostState.hostMinor >= 20;
  if (!manages && mark.length > 0) sections.push({ items: mark });
  if (manages) {
    const rename: MenuItem[] =
      hostState.hostMinor >= 33
        ? [
            {
              key: 'rename',
              label: 'Rename…',
              icon: icon(Pencil),
              // Pushed, even from the sidebar, which opens screens in
              // place of the one beside it: the sheet goes over that one,
              // and leaving puts it back.
              onPress: () => router.push(`/rename/${conn.id}/${encodeURIComponent(worktree.id)}`),
            },
          ]
        : [];
    sections.push({
      items: [
        ...rename,
        worktree.pinned
          ? {
              key: 'unpin',
              label: 'Unpin',
              icon: icon(PinOff),
              onPress: () => void conn.pinWorktree(worktree.id, false).catch(reason).then(failed("Couldn't unpin it")),
            }
          : {
              key: 'pin',
              label: 'Pin',
              icon: icon(Pin),
              onPress: () => void conn.pinWorktree(worktree.id, true).catch(reason).then(failed("Couldn't pin it")),
            },
        ...mark,
      ],
    });
  }

  if (!pr) {
    // Not before 1.16, which opens them.
    if (!primary && hostState.hostMinor >= 16) {
      sections.push({
        items: [
          {
            key: 'open-pr',
            label: 'Open pull request…',
            icon: icon(PullRequestGlyph),
            onPress: () => open(`/pull-request/${conn.id}/${encodeURIComponent(worktree.id)}`),
          },
        ],
      });
    }
  } else {
    const can = prActions(pr, hostState.hostMinor);

    // Handed to the agent: off to its conversation to watch it work, or word
    // of where the handing stands when there is none to open.
    const handOver = async (kind: 'checks' | 'comments') => {
      const handed = await (kind === 'checks' ? conn.fixChecks(worktree.id) : conn.addressComments(worktree.id)).catch(
        (e: unknown) => ({ why: reason(e) }),
      );
      const what = kind === 'checks' ? 'failed checks' : 'review comments';
      if ('why' in handed) {
        Alert.alert(kind === 'checks' ? "Couldn't hand over the checks" : "Couldn't hand over the comments", handed.why);
        return;
      }
      if (handed.starting) {
        Alert.alert('Starting the agent', `ket on ${host} is still starting the agent. It gets the ${what} once it's up.`);
        return;
      }
      const terminal = agentTerminal(worktree);
      if (terminal) open(`/session/${conn.id}/${terminal.id.toString()}`);
      else Alert.alert('Handed over', `The agent on ${host} has the ${what}.`);
    };

    const items: MenuItem[] = [];
    if (can.fix) items.push({ key: 'fix', label: 'Fix checks with agent', icon: icon(CheckCircle), onPress: () => void handOver('checks') });
    if (can.address)
      items.push({ key: 'address', label: 'Address comments with agent', icon: icon(Comment), onPress: () => void handOver('comments') });
    if (can.update)
      items.push({
        key: 'update',
        label: 'Update branch',
        icon: icon(Refresh),
        onPress: () => void conn.updatePullRequest(worktree.id).catch(reason).then(failed("Couldn't update the branch")),
      });
    if (can.ready)
      items.push({
        key: 'ready',
        label: 'Ready for review',
        icon: icon(Check),
        onPress: () => void conn.readyPullRequest(worktree.id).catch(reason).then(failed("Couldn't mark it ready")),
      });
    if (can.reopen)
      items.push({
        key: 'reopen',
        label: 'Reopen pull request',
        icon: icon(PullRequestGlyph),
        onPress: () => void conn.reopenPullRequest(worktree.id).catch(reason).then(failed("Couldn't reopen it")),
      });
    if (can.merge)
      items.push({
        key: 'merge',
        label: 'Merge pull request…',
        icon: icon(Merge),
        onPress: () =>
          askMerge(pr, (method) => {
            void (async () => {
              if (!(await confirmIdentity(`Merge #${pr.number.toString()}`))) return;
              failed("Couldn't merge")(await conn.mergePullRequest(worktree.id, method).catch(reason));
            })();
          }),
      });
    if (items.length > 0) sections.push({ title: `Pull request #${pr.number.toString()}`, items });
  }

  if (manages) {
    // Asked first, as the desktop's dialog asks; then it is you. The row goes
    // at once — the desktop answers only once git has walked the checkout,
    // which can take a while — and comes back if the delete is refused. A
    // delete git refuses because work would be lost says so, and offers to
    // go anyway; the row stays out while that is asked.
    const remove = async (force: boolean) => {
      going(worktree.id, true);
      const result = await conn.deleteWorktree(worktree.id, force).catch((e: unknown) => ({ why: reason(e), losesWork: false }));
      if ('keptBranch' in result) {
        if (result.keptBranch) {
          Alert.alert('Worktree deleted', `Kept the branch ${result.keptBranch}: it has unmerged commits.`);
        }
        return;
      }
      if (result.losesWork && !force) {
        const back = () => going(worktree.id, false);
        Alert.alert(
          `Delete ${name} anyway?`,
          sentence(result.why),
          [
            { text: 'Cancel', style: 'cancel', onPress: back },
            { text: 'Delete anyway', style: 'destructive', onPress: () => void remove(true) },
          ],
          { cancelable: true, onDismiss: back },
        );
        return;
      }
      going(worktree.id, false);
      Alert.alert(`Couldn't delete ${name}`, sentence(result.why));
    };
    sections.push({
      items: [
        {
          key: 'delete',
          label: 'Delete…',
          danger: true,
          icon: icon(Trash, t.failed),
          onPress: () =>
            Alert.alert(`Delete ${name}?`, 'Its checkout and its terminals go. Unmerged commits keep their branch.', [
              { text: 'Cancel', style: 'cancel' },
              {
                text: 'Delete',
                style: 'destructive',
                onPress: () => {
                  void (async () => {
                    if (!(await confirmIdentity(`Delete ${name}`))) return;
                    await remove(false);
                  })();
                },
              },
            ]),
        },
      ],
    });
  }
  return sections;
}
