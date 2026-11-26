// The phone protocol's application contract (Epic 5b.2).
//
// Everything here travels inside a Noise session between a phone and a ket
// host, so the relay in between sees none of it. The Rust host and the
// TypeScript app both generate from this one file.
//
// Compatibility: `major` changes break, `minor` ones add. A peer tolerates a
// minor it does not know — proto3 ignores unknown fields — and refuses a
// major with `ErrorCode.UNSUPPORTED_VERSION` before anything else is sent.
//
// 1.6 adds a worktree's figures, pairing that a person approves on the
// desktop (PairingPending), and names every prompt: `Question.id` and
// `Question.terminal_id`, which an `Answer` must return as `question_id` and
// `terminal_id`. That one is not only additive — a 1.6 host refuses an
// answer without a `question_id`, saying to update the phone, rather than
// press keys into a prompt it cannot be sure the phone saw. Everything else
// a 1.5 phone does still works.
//
// 1.7 adds GetConversation, an agent's session as a conversation; StartWork,
// new work from a phone; GetChanges and GetDiff, reviewing a worktree;
// MergeWorktree; GetSnippets; a worktree's subagents; and the messages
// still queued for an agent.
//
// 1.8 adds the prompts that are not permissions — a question Claude asks
// with options (`Question.asks`) and a plan it wants approved
// (`Question.plan`) — and Respond, which answers them; and the agents the
// desktop can start (`HostSnapshot.agents`), for StartWork to choose from.
//
// 1.9 adds a project's backlog: how many notes are open
// (`ProjectSummary.backlog`), GetBacklog, EditBacklog and StartNote.
//
// 1.10 adds a note's priority: `BacklogNote.priority`, and
// `EditBacklog.priority` to change it with a SAVE.
//
// 1.11 adds the pull request a worktree's branch is open as on GitHub, with
// its checks (`WorktreeSummary.pull_request`), and FixChecks, which hands the
// failed ones to the worktree's agent and is answered with ChecksHandedOver.
//
// 1.12 adds photos and files sent with a message, SendFiles, and how many
// images a message read back carried, `Turn.images`.
//
// 1.13 adds every prompt a worktree's panes are waiting on
// (`WorktreeSummary.questions`), so a phone showing one of several agents in
// a worktree sees that agent's prompt and not only the newest.
//
// 1.14 adds SendReview: comments on a worktree's diff, sent to its agent as
// one message, as the desktop's Send to agent sends its review.
//
// 1.15 adds start options and fresh sessions in existing worktrees.
//
// 1.16 adds GetPullRequestDraft and OpenPullRequest: a worktree's branch
// pushed and opened as a pull request, as the desktop's Pull request… does.
//
// 1.17 adds the host's current reachable addresses to HostSnapshot. A phone
// keeps them for later connections; its pairing code still carries one URL.
//
// 1.18 is Sleep.
//
// 1.19 adds pull request actions: MergePullRequest, UpdatePullRequest,
// ReadyPullRequest, ReopenPullRequest, and AddressComments, which hands
// unresolved review comments to the worktree's agent the way FixChecks
// hands failed checks. PullRequestSummary gains state, base, behind,
// unresolved comments, and the merge methods the repository allows.
//
// 1.20 adds whether a worktree is pinned (`WorktreeSummary.pinned`),
// PinWorktree, and DeleteWorktree, answered with WorktreeDeleted — the
// worktree menu's Pin and Delete. Neither touches a repository's own
// checkout.
//
// 1.21 adds Claude Code's agent view: a terminal showing it lists the
// background sessions it holds (`TerminalSummary.agent_view`), a phone reads
// one of them (`GetConversation.session`), and OpenAgentSession opens one in
// the agent view and types a message to it.
//
// 1.22 adds `AgentView.listed`, so an agent view whose sessions the desktop
// is still reading is not shown as having none.
//
// 1.23 adds a worktree's asleep tabs (`WorktreeSummary.asleep`): agent tabs
// the desktop stopped while keeping their sessions. WakeTerminal starts one
// again, answered with TerminalWoken naming the terminal it now is.
//
// 1.24 adds an agent's modes (`TerminalSummary.modes`): Claude's permission
// mode, OpenCode's agent, Codex's collaboration mode, with the one it is in.
// SetMode asks the desktop to put the agent in one, answered with Ack once
// the agent reports it.

// 1.25 adds what an agent view session is doing and when it last did
// anything (`BackgroundSession.doing`, `active_at_ms`), and starting a new
// one: OpenAgentSession with no session.

// 1.26 adds the model and reasoning effort a terminal's agent runs with
// (`TerminalSummary.settings`), and changing them: SetModel.

// 1.27 adds a project's icon (`ProjectSummary.icon`), so the phone's badge
// shows the glyph the desktop's does.

// 1.28 adds a terminal whose agent session's prompt cache has gone cold
// (`TerminalSummary.cold`), and StartFresh, which opens a fresh session
// beside it the way the desktop's own question does, answered with
// FreshStarted naming the fresh session's terminal.

// 1.29 adds what each terminal is doing (`TerminalSummary.activity`), and
// the program a shell is running with when it started
// (`TerminalSummary.running`, `running_since_ms`), so a phone can list a
// worktree's tabs as its desktop's tab strip has them.
//
// 1.30 adds whether a terminal's agent is compacting its conversation
// (`TerminalSummary.compacting`).

// 1.31 adds the colour a terminal's session was given with Claude's `/color`
// (`TerminalSummary.color`, `WorktreeSummary.agent_color`).

// 1.32 adds a terminal whose agent has answered and nobody has looked since
// (`TerminalSummary.unread`), and MarkSeen, which a phone sends as it shows
// the terminal's chat, answered with Ack.

// 1.33 adds RenameWorktree, the worktree menu's Rename.

// 1.34 adds RegisterPush and UnregisterPush, where the host sends a phone
// a notification while it is not connected and for what, and `Device.push`.

// 1.35 adds MarkUnread, the worktree menu's Mark as Unread, and
// `WorktreeSummary.unread`, set while a worktree is marked.
//
// 1.36 adds GetAutomations and EditAutomation: the automations, work that
// starts itself on a schedule, and pausing, resuming, starting now or
// deleting one.
//
// 1.37 adds blackout hours (`HostSnapshot.blackout`): hours set on the
// desktop when ket is shut off on every paired device. During one a phone is
// sent a snapshot with nothing in it but the blackout, and whatever it asks
// for is refused with `ErrorCode.BLACKOUT`. Only the desktop lifts it.

// 1.38 adds a session stopped because its plan ran out
// (`TerminalSummary.limit`, `AsleepTerminal.limit`), and PUSH_KIND_LIMITS,
// for hearing of one and of its limit resetting.

// 1.39 adds PreviewSwitch and SwitchAgent: a terminal's task handed to
// another agent, with a brief of its conversation the phone shows before it
// is sent, answered with SwitchPreview and AgentSwitched.

// 1.40 adds what a terminal's agent left running in the background — a
// build, a test run, a dev server — and what of that ended a moment ago
// (`TerminalSummary.jobs`), and StopJob, which ends one.

// @generated by protoc-gen-es v2.15.0 with parameter "target=js+dts,import_extension=js"
// @generated from file ket/remote/v1/remote.proto (package ket.remote.v1, syntax proto3)
/* eslint-disable */

import { enumDesc, fileDesc, messageDesc, tsEnum } from "@bufbuild/protobuf/codegenv2";

/**
 * Describes the file ket/remote/v1/remote.proto.
 */
export const file_ket_remote_v1_remote = /*@__PURE__*/
  fileDesc("ChprZXQvcmVtb3RlL3YxL3JlbW90ZS5wcm90bxINa2V0LnJlbW90ZS52MSLnIQoIRW52ZWxvcGUSDQoFbWFqb3IYASABKA0SDQoFbWlub3IYAiABKA0SEgoKcmVxdWVzdF9pZBgDIAEoBBIRCglzdHJlYW1faWQYBCABKAQSEgoKaG9zdF9lcG9jaBgFIAEoBBILCgNzZXEYBiABKAQSFwoPaWRlbXBvdGVuY3lfa2V5GAcgASgJEiUKBWhlbGxvGAogASgLMhQua2V0LnJlbW90ZS52MS5IZWxsb0gAEikKB3dlbGNvbWUYCyABKAsyFi5rZXQucmVtb3RlLnYxLldlbGNvbWVIABIlCgVlcnJvchgMIAEoCzIULmtldC5yZW1vdGUudjEuRXJyb3JIABIhCgNhY2sYDSABKAsyEi5rZXQucmVtb3RlLnYxLkFja0gAEjgKD3BhaXJpbmdfcGVuZGluZxgOIAEoCzIdLmtldC5yZW1vdGUudjEuUGFpcmluZ1BlbmRpbmdIABIvCghzbmFwc2hvdBgUIAEoCzIbLmtldC5yZW1vdGUudjEuSG9zdFNuYXBzaG90SAASLQoJc3Vic2NyaWJlGB4gASgLMhgua2V0LnJlbW90ZS52MS5TdWJzY3JpYmVIABIxCgt1bnN1YnNjcmliZRgfIAEoCzIaLmtldC5yZW1vdGUudjEuVW5zdWJzY3JpYmVIABI3CgpjaGVja3BvaW50GCAgASgLMiEua2V0LnJlbW90ZS52MS5UZXJtaW5hbENoZWNrcG9pbnRIABItCgVjaHVuaxghIAEoCzIcLmtldC5yZW1vdGUudjEuVGVybWluYWxDaHVua0gAEi0KBXJlc2V0GCIgASgLMhwua2V0LnJlbW90ZS52MS5UZXJtaW5hbFJlc2V0SAASLwoGY2xvc2VkGCMgASgLMh0ua2V0LnJlbW90ZS52MS5UZXJtaW5hbENsb3NlZEgAEiUKBWlucHV0GCggASgLMhQua2V0LnJlbW90ZS52MS5JbnB1dEgAEicKBnJlc2l6ZRgpIAEoCzIVLmtldC5yZW1vdGUudjEuUmVzaXplSAASJQoFbGVhc2UYKiABKAsyFC5rZXQucmVtb3RlLnYxLkxlYXNlSAASJwoGc2lnbmFsGCsgASgLMhUua2V0LnJlbW90ZS52MS5TaWduYWxIABIyCgxsaXN0X2RldmljZXMYMiABKAsyGi5rZXQucmVtb3RlLnYxLkxpc3REZXZpY2VzSAASLAoHZGV2aWNlcxgzIAEoCzIZLmtldC5yZW1vdGUudjEuRGV2aWNlTGlzdEgAEjQKDXJldm9rZV9kZXZpY2UYNCABKAsyGy5rZXQucmVtb3RlLnYxLlJldm9rZURldmljZUgAEicKBmFuc3dlchg8IAEoCzIVLmtldC5yZW1vdGUudjEuQW5zd2VySAASOgoQZ2V0X2NvbnZlcnNhdGlvbhhGIAEoCzIeLmtldC5yZW1vdGUudjEuR2V0Q29udmVyc2F0aW9uSAASMwoMY29udmVyc2F0aW9uGEcgASgLMhsua2V0LnJlbW90ZS52MS5Db252ZXJzYXRpb25IABIuCgpzdGFydF93b3JrGEggASgLMhgua2V0LnJlbW90ZS52MS5TdGFydFdvcmtIABIwCgtnZXRfY2hhbmdlcxhJIAEoCzIZLmtldC5yZW1vdGUudjEuR2V0Q2hhbmdlc0gAEikKB2NoYW5nZXMYSiABKAsyFi5rZXQucmVtb3RlLnYxLkNoYW5nZXNIABIqCghnZXRfZGlmZhhLIAEoCzIWLmtldC5yZW1vdGUudjEuR2V0RGlmZkgAEiMKBGRpZmYYTCABKAsyEy5rZXQucmVtb3RlLnYxLkRpZmZIABI2Cg5tZXJnZV93b3JrdHJlZRhNIAEoCzIcLmtldC5yZW1vdGUudjEuTWVyZ2VXb3JrdHJlZUgAEjIKDGdldF9zbmlwcGV0cxhOIAEoCzIaLmtldC5yZW1vdGUudjEuR2V0U25pcHBldHNIABIuCghzbmlwcGV0cxhPIAEoCzIaLmtldC5yZW1vdGUudjEuU25pcHBldExpc3RIABIpCgdyZXNwb25kGFAgASgLMhYua2V0LnJlbW90ZS52MS5SZXNwb25kSAASMAoLZ2V0X2JhY2tsb2cYWiABKAsyGS5rZXQucmVtb3RlLnYxLkdldEJhY2tsb2dIABIpCgdiYWNrbG9nGFsgASgLMhYua2V0LnJlbW90ZS52MS5CYWNrbG9nSAASMgoMZWRpdF9iYWNrbG9nGFwgASgLMhoua2V0LnJlbW90ZS52MS5FZGl0QmFja2xvZ0gAEi4KCnN0YXJ0X25vdGUYXSABKAsyGC5rZXQucmVtb3RlLnYxLlN0YXJ0Tm90ZUgAEi4KCmZpeF9jaGVja3MYZCABKAsyGC5rZXQucmVtb3RlLnYxLkZpeENoZWNrc0gAEj0KEmNoZWNrc19oYW5kZWRfb3ZlchhlIAEoCzIfLmtldC5yZW1vdGUudjEuQ2hlY2tzSGFuZGVkT3ZlckgAEi4KCnNlbmRfZmlsZXMYZiABKAsyGC5rZXQucmVtb3RlLnYxLlNlbmRGaWxlc0gAEjAKC3NlbmRfcmV2aWV3GGcgASgLMhkua2V0LnJlbW90ZS52MS5TZW5kUmV2aWV3SAASOwoRZ2V0X3N0YXJ0X29wdGlvbnMYaCABKAsyHi5rZXQucmVtb3RlLnYxLkdldFN0YXJ0T3B0aW9uc0gAEjQKDXN0YXJ0X29wdGlvbnMYaSABKAsyGy5rZXQucmVtb3RlLnYxLlN0YXJ0T3B0aW9uc0gAEjQKDXN0YXJ0X3Nlc3Npb24YaiABKAsyGy5rZXQucmVtb3RlLnYxLlN0YXJ0U2Vzc2lvbkgAEkQKFmdldF9wdWxsX3JlcXVlc3RfZHJhZnQYayABKAsyIi5rZXQucmVtb3RlLnYxLkdldFB1bGxSZXF1ZXN0RHJhZnRIABI9ChJwdWxsX3JlcXVlc3RfZHJhZnQYbCABKAsyHy5rZXQucmVtb3RlLnYxLlB1bGxSZXF1ZXN0RHJhZnRIABI7ChFvcGVuX3B1bGxfcmVxdWVzdBhtIAEoCzIeLmtldC5yZW1vdGUudjEuT3BlblB1bGxSZXF1ZXN0SAASPwoTcHVsbF9yZXF1ZXN0X29wZW5lZBhuIAEoCzIgLmtldC5yZW1vdGUudjEuUHVsbFJlcXVlc3RPcGVuZWRIABI9ChJtZXJnZV9wdWxsX3JlcXVlc3QYbyABKAsyHy5rZXQucmVtb3RlLnYxLk1lcmdlUHVsbFJlcXVlc3RIABI/ChNwdWxsX3JlcXVlc3RfbWVyZ2VkGHAgASgLMiAua2V0LnJlbW90ZS52MS5QdWxsUmVxdWVzdE1lcmdlZEgAEj8KE3VwZGF0ZV9wdWxsX3JlcXVlc3QYcSABKAsyIC5rZXQucmVtb3RlLnYxLlVwZGF0ZVB1bGxSZXF1ZXN0SAASQQoUcHVsbF9yZXF1ZXN0X3VwZGF0ZWQYciABKAsyIS5rZXQucmVtb3RlLnYxLlB1bGxSZXF1ZXN0VXBkYXRlZEgAEj0KEnJlYWR5X3B1bGxfcmVxdWVzdBhzIAEoCzIfLmtldC5yZW1vdGUudjEuUmVhZHlQdWxsUmVxdWVzdEgAEkEKFHB1bGxfcmVxdWVzdF9yZWFkaWVkGHQgASgLMiEua2V0LnJlbW90ZS52MS5QdWxsUmVxdWVzdFJlYWRpZWRIABI/ChNyZW9wZW5fcHVsbF9yZXF1ZXN0GHUgASgLMiAua2V0LnJlbW90ZS52MS5SZW9wZW5QdWxsUmVxdWVzdEgAEkMKFXB1bGxfcmVxdWVzdF9yZW9wZW5lZBh2IAEoCzIiLmtldC5yZW1vdGUudjEuUHVsbFJlcXVlc3RSZW9wZW5lZEgAEjoKEGFkZHJlc3NfY29tbWVudHMYdyABKAsyHi5rZXQucmVtb3RlLnYxLkFkZHJlc3NDb21tZW50c0gAEkEKFGNvbW1lbnRzX2hhbmRlZF9vdmVyGHggASgLMiEua2V0LnJlbW90ZS52MS5Db21tZW50c0hhbmRlZE92ZXJIABIyCgxwaW5fd29ya3RyZWUYeSABKAsyGi5rZXQucmVtb3RlLnYxLlBpbldvcmt0cmVlSAASOAoPZGVsZXRlX3dvcmt0cmVlGHogASgLMh0ua2V0LnJlbW90ZS52MS5EZWxldGVXb3JrdHJlZUgAEjoKEHdvcmt0cmVlX2RlbGV0ZWQYeyABKAsyHi5rZXQucmVtb3RlLnYxLldvcmt0cmVlRGVsZXRlZEgAEj0KEm9wZW5fYWdlbnRfc2Vzc2lvbhh8IAEoCzIfLmtldC5yZW1vdGUudjEuT3BlbkFnZW50U2Vzc2lvbkgAEjQKDXdha2VfdGVybWluYWwYfSABKAsyGy5rZXQucmVtb3RlLnYxLldha2VUZXJtaW5hbEgAEjYKDnRlcm1pbmFsX3dva2VuGH4gASgLMhwua2V0LnJlbW90ZS52MS5UZXJtaW5hbFdva2VuSAASKgoIc2V0X21vZGUYfyABKAsyFi5rZXQucmVtb3RlLnYxLlNldE1vZGVIABItCglzZXRfbW9kZWwYgAEgASgLMhcua2V0LnJlbW90ZS52MS5TZXRNb2RlbEgAEjEKC3N0YXJ0X2ZyZXNoGIEBIAEoCzIZLmtldC5yZW1vdGUudjEuU3RhcnRGcmVzaEgAEjUKDWZyZXNoX3N0YXJ0ZWQYggEgASgLMhsua2V0LnJlbW90ZS52MS5GcmVzaFN0YXJ0ZWRIABItCgltYXJrX3NlZW4YgwEgASgLMhcua2V0LnJlbW90ZS52MS5NYXJrU2VlbkgAEjkKD3JlbmFtZV93b3JrdHJlZRiEASABKAsyHS5rZXQucmVtb3RlLnYxLlJlbmFtZVdvcmt0cmVlSAASNQoNcmVnaXN0ZXJfcHVzaBiFASABKAsyGy5rZXQucmVtb3RlLnYxLlJlZ2lzdGVyUHVzaEgAEjkKD3VucmVnaXN0ZXJfcHVzaBiGASABKAsyHS5rZXQucmVtb3RlLnYxLlVucmVnaXN0ZXJQdXNoSAASMQoLbWFya191bnJlYWQYhwEgASgLMhkua2V0LnJlbW90ZS52MS5NYXJrVW5yZWFkSAASOQoPZ2V0X2F1dG9tYXRpb25zGIgBIAEoCzIdLmtldC5yZW1vdGUudjEuR2V0QXV0b21hdGlvbnNIABIyCgthdXRvbWF0aW9ucxiJASABKAsyGi5rZXQucmVtb3RlLnYxLkF1dG9tYXRpb25zSAASOQoPZWRpdF9hdXRvbWF0aW9uGIoBIAEoCzIdLmtldC5yZW1vdGUudjEuRWRpdEF1dG9tYXRpb25IABI3Cg5wcmV2aWV3X3N3aXRjaBiLASABKAsyHC5rZXQucmVtb3RlLnYxLlByZXZpZXdTd2l0Y2hIABI3Cg5zd2l0Y2hfcHJldmlldxiMASABKAsyHC5rZXQucmVtb3RlLnYxLlN3aXRjaFByZXZpZXdIABIzCgxzd2l0Y2hfYWdlbnQYjQEgASgLMhoua2V0LnJlbW90ZS52MS5Td2l0Y2hBZ2VudEgAEjcKDmFnZW50X3N3aXRjaGVkGI4BIAEoCzIcLmtldC5yZW1vdGUudjEuQWdlbnRTd2l0Y2hlZEgAEisKCHN0b3Bfam9iGI8BIAEoCzIWLmtldC5yZW1vdGUudjEuU3RvcEpvYkgAQgkKB3BheWxvYWQiWwoFSGVsbG8SEwoLZGV2aWNlX25hbWUYASABKAkSEwoLYXBwX3ZlcnNpb24YAiABKAkSFAoMcmVzdW1lX2Vwb2NoGAMgASgEEhIKCnJlc3VtZV9zZXEYBCABKAQiNwoOUGFpcmluZ1BlbmRpbmcSDAoEY29kZRgBIAEoCRIXCg90aW1lb3V0X3NlY29uZHMYAiABKA0iQQoHV2VsY29tZRIRCglob3N0X25hbWUYASABKAkSEgoKaG9zdF9lcG9jaBgCIAEoBBIPCgdyZXN1bWVkGAMgASgIIkAKBUVycm9yEiYKBGNvZGUYASABKA4yGC5rZXQucmVtb3RlLnYxLkVycm9yQ29kZRIPCgdtZXNzYWdlGAIgASgJIgUKA0FjayLjAQoMSG9zdFNuYXBzaG90Ei8KCHByb2plY3RzGAEgAygLMh0ua2V0LnJlbW90ZS52MS5Qcm9qZWN0U3VtbWFyeRIrCgV1c2FnZRgCIAMoCzIcLmtldC5yZW1vdGUudjEuUHJvdmlkZXJVc2FnZRIOCgZhZ2VudHMYAyADKAkSJwoHcGFsZXR0ZRgEIAEoCzIWLmtldC5yZW1vdGUudjEuUGFsZXR0ZRIRCglhZGRyZXNzZXMYBSADKAkSKQoIYmxhY2tvdXQYBiABKAsyFy5rZXQucmVtb3RlLnYxLkJsYWNrb3V0InUKCEJsYWNrb3V0EhAKCHVudGlsX21zGAEgASgEEi4KB3dpbmRvd3MYAiADKAsyHS5rZXQucmVtb3RlLnYxLkJsYWNrb3V0V2luZG93Eg4KBnJlcGVhdBgDIAEoCRIXCg9sZXRfaW5fdW50aWxfbXMYBCABKAQiMgoOQmxhY2tvdXRXaW5kb3cSEAoIc3RhcnRfbXMYASABKAQSDgoGZW5kX21zGAIgASgEIqEDCgdQYWxldHRlEg0KBWxpZ2h0GAEgASgIEhAKCGJhY2tkcm9wGAIgASgJEg8KB3N1cmZhY2UYAyABKAkSDgoGc3Vua2VuGAQgASgJEhAKCGVsZXZhdGVkGAUgASgJEg0KBWhvdmVyGAYgASgJEhEKCXNlbGVjdGlvbhgHIAEoCRIOCgZib3JkZXIYCCABKAkSDAoEcnVsZRgJIAEoCRIOCgZhY2NlbnQYCiABKAkSEQoJb25fYWNjZW50GAsgASgJEgwKBHRleHQYDCABKAkSCwoDZGltGA0gASgJEg4KBm1hcmtlchgOIAEoCRIPCgdydW5uaW5nGA8gASgJEhEKCWF0dGVudGlvbhgQIAEoCRIOCgZmYWlsZWQYESABKAkSDwoHbWVyZ2luZxgSIAEoCRINCgVhZGRlZBgTIAEoCRIPCgdyZW1vdmVkGBQgASgJEhAKCG1vZGlmaWVkGBUgASgJEhIKCnF1b3RhX3dhcm0YFiABKAkSEQoJcXVvdGFfaG90GBcgASgJEhYKDnF1b3RhX2NyaXRpY2FsGBggASgJIq8BCg1Qcm92aWRlclVzYWdlEhAKCHByb3ZpZGVyGAEgASgJEioKBnN0YXR1cxgCIAEoDjIaLmtldC5yZW1vdGUudjEuVXNhZ2VTdGF0dXMSKwoHd2luZG93cxgDIAMoCzIaLmtldC5yZW1vdGUudjEuVXNhZ2VXaW5kb3cSDAoEcGxhbhgEIAEoCRIVCg1mZXRjaGVkX2F0X21zGAUgASgEEg4KBnJlYXNvbhgGIAEoCSJHCgtVc2FnZVdpbmRvdxIMCgRuYW1lGAEgASgJEhQKDHVzZWRfcGVyY2VudBgCIAEoAhIUCgxyZXNldHNfYXRfbXMYAyABKAQiiwEKDlByb2plY3RTdW1tYXJ5EgoKAmlkGAEgASgJEgwKBG5hbWUYAiABKAkSMQoJd29ya3RyZWVzGAMgAygLMh4ua2V0LnJlbW90ZS52MS5Xb3JrdHJlZVN1bW1hcnkSDQoFY29sb3IYBCABKAkSDwoHYmFja2xvZxgFIAEoDRIMCgRpY29uGAYgASgJIswECg9Xb3JrdHJlZVN1bW1hcnkSCgoCaWQYASABKAkSDAoEbmFtZRgCIAEoCRIOCgZicmFuY2gYAyABKAkSEwoLYWdlbnRfc3RhdGUYBCABKAkSMQoJdGVybWluYWxzGAUgAygLMh4ua2V0LnJlbW90ZS52MS5UZXJtaW5hbFN1bW1hcnkSKQoIYWN0aXZpdHkYBiABKA4yFy5rZXQucmVtb3RlLnYxLkFjdGl2aXR5Eg0KBWFnZW50GAcgASgJEikKCHF1ZXN0aW9uGAggASgLMhcua2V0LnJlbW90ZS52MS5RdWVzdGlvbhIVCg1maWxlc19jaGFuZ2VkGAkgASgNEhMKC2xpbmVzX2FkZGVkGAogASgNEhUKDWxpbmVzX3JlbW92ZWQYCyABKA0SFQoNY29tbWl0c19haGVhZBgMIAEoDRIMCgRiYXNlGA0gASgJEjEKCXN1YmFnZW50cxgOIAMoCzIeLmtldC5yZW1vdGUudjEuU3ViYWdlbnRTdW1tYXJ5EjcKDHB1bGxfcmVxdWVzdBgPIAEoCzIhLmtldC5yZW1vdGUudjEuUHVsbFJlcXVlc3RTdW1tYXJ5EioKCXF1ZXN0aW9ucxgQIAMoCzIXLmtldC5yZW1vdGUudjEuUXVlc3Rpb24SDgoGcGlubmVkGBEgASgIEi0KBmFzbGVlcBgSIAMoCzIdLmtldC5yZW1vdGUudjEuQXNsZWVwVGVybWluYWwSEwoLYWdlbnRfY29sb3IYEyABKAkSDgoGdW5yZWFkGBQgASgIIvsDChJQdWxsUmVxdWVzdFN1bW1hcnkSDgoGbnVtYmVyGAEgASgEEgsKA3VybBgCIAEoCRINCgVkcmFmdBgDIAEoCBI4CgZyZXZpZXcYBCABKA4yKC5rZXQucmVtb3RlLnYxLlB1bGxSZXF1ZXN0U3VtbWFyeS5SZXZpZXcSKwoGY2hlY2tzGAUgAygLMhsua2V0LnJlbW90ZS52MS5DaGVja1N1bW1hcnkSNgoFc3RhdGUYBiABKA4yJy5rZXQucmVtb3RlLnYxLlB1bGxSZXF1ZXN0U3VtbWFyeS5TdGF0ZRIMCgRiYXNlGAcgASgJEg4KBmJlaGluZBgIIAEoCBIbChN1bnJlc29sdmVkX2NvbW1lbnRzGAkgASgNEhwKFG1lcmdlX2NvbW1pdF9hbGxvd2VkGAogASgIEhwKFHNxdWFzaF9tZXJnZV9hbGxvd2VkGAsgASgIEhwKFHJlYmFzZV9tZXJnZV9hbGxvd2VkGAwgASgIIloKBlJldmlldxIWChJSRVZJRVdfVU5TUEVDSUZJRUQQABIMCghBUFBST1ZFRBABEhUKEUNIQU5HRVNfUkVRVUVTVEVEEAISEwoPUkVWSUVXX1JFUVVJUkVEEAMiKQoFU3RhdGUSCAoET1BFThAAEgoKBkNMT1NFRBABEgoKBk1FUkdFRBACIq0BCgxDaGVja1N1bW1hcnkSDAoEbmFtZRgBIAEoCRIwCgVzdGF0ZRgCIAEoDjIhLmtldC5yZW1vdGUudjEuQ2hlY2tTdW1tYXJ5LlN0YXRlEgsKA3VybBgDIAEoCSJQCgVTdGF0ZRIVChFTVEFURV9VTlNQRUNJRklFRBAAEgoKBlBBU1NFRBABEgoKBkZBSUxFRBACEgsKB1BFTkRJTkcQAxILCgdTS0lQUEVEEAQiywEKD1N1YmFnZW50U3VtbWFyeRIKCgJpZBgBIAEoCRINCgVsYWJlbBgCIAEoCRINCgVtb2RlbBgDIAEoCRIzCgVzdGF0ZRgEIAEoDjIkLmtldC5yZW1vdGUudjEuU3ViYWdlbnRTdW1tYXJ5LlN0YXRlEhUKDXN0YXJ0ZWRfYXRfbXMYBSABKAQiQgoFU3RhdGUSFQoRU1RBVEVfVU5TUEVDSUZJRUQQABILCgdXT1JLSU5HEAESCwoHQkxPQ0tFRBACEggKBElETEUQAyLDAQoIUXVlc3Rpb24SDAoEdG9vbBgBIAEoCRIPCgdzdWJqZWN0GAIgASgJEhIKCmFuc3dlcmFibGUYAyABKAgSDgoGYWx3YXlzGAQgASgIEgoKAmlkGAUgASgEEhMKC3Rlcm1pbmFsX2lkGAYgASgEEiEKBHJpc2sYByABKAsyEy5rZXQucmVtb3RlLnYxLlJpc2sSIgoEYXNrcxgIIAMoCzIULmtldC5yZW1vdGUudjEuQXNrZWQSDAoEcGxhbhgJIAEoCCKbAQoFQXNrZWQSEAoIcXVlc3Rpb24YASABKAkSDgoGaGVhZGVyGAIgASgJEiwKB2Nob2ljZXMYAyADKAsyGy5rZXQucmVtb3RlLnYxLkFza2VkLkNob2ljZRIUCgxtdWx0aV9zZWxlY3QYBCABKAgaLAoGQ2hvaWNlEg0KBWxhYmVsGAEgASgJEhMKC2Rlc2NyaXB0aW9uGAIgASgJIikKBFJpc2sSDQoFbGV2ZWwYASABKAkSEgoKY29uZmlkZW5jZRgCIAEoAiKOAQoOQXNsZWVwVGVybWluYWwSCwoDa2V5GAEgASgJEg0KBXRpdGxlGAIgASgJEg0KBWFnZW50GAMgASgJEhAKCHNpbmNlX21zGAQgASgEEhMKC2ZyZWVkX2J5dGVzGAUgASgEEioKBWxpbWl0GAYgASgLMhsua2V0LnJlbW90ZS52MS5TZXNzaW9uTGltaXQitAIKDFNlc3Npb25MaW1pdBIwCgVzY29wZRgBIAEoDjIhLmtldC5yZW1vdGUudjEuU2Vzc2lvbkxpbWl0LlNjb3BlEjIKBndpbmRvdxgCIAEoDjIiLmtldC5yZW1vdGUudjEuU2Vzc2lvbkxpbWl0LldpbmRvdxIQCghzaW5jZV9tcxgDIAEoBBIUCgxyZXNldHNfYXRfbXMYBCABKAQSDAoEc2FpZBgFIAEoCSI/CgVTY29wZRIVChFTQ09QRV9VTlNQRUNJRklFRBAAEg4KClNDT1BFX1BMQU4QARIPCgtTQ09QRV9NT0RFTBACIkcKBldpbmRvdxIWChJXSU5ET1dfVU5TUEVDSUZJRUQQABISCg5XSU5ET1dfU0VTU0lPThABEhEKDVdJTkRPV19XRUVLTFkQAiIwCgxXYWtlVGVybWluYWwSEwoLd29ya3RyZWVfaWQYASABKAkSCwoDa2V5GAIgASgJIiQKDVRlcm1pbmFsV29rZW4SEwoLdGVybWluYWxfaWQYASABKAQi0gMKD1Rlcm1pbmFsU3VtbWFyeRIKCgJpZBgBIAEoBBINCgV0aXRsZRgCIAEoCRINCgVhZ2VudBgDIAEoCRIOCgZjbG9zZWQYBCABKAgSLAoKYWdlbnRfdmlldxgFIAEoCzIYLmtldC5yZW1vdGUudjEuQWdlbnRWaWV3EigKBW1vZGVzGAYgASgLMhkua2V0LnJlbW90ZS52MS5BZ2VudE1vZGVzEi4KCHNldHRpbmdzGAcgASgLMhwua2V0LnJlbW90ZS52MS5BZ2VudFNldHRpbmdzEiYKBGNvbGQYCCABKAsyGC5rZXQucmVtb3RlLnYxLkNvbGRDYWNoZRIpCghhY3Rpdml0eRgJIAEoDjIXLmtldC5yZW1vdGUudjEuQWN0aXZpdHkSDwoHcnVubmluZxgKIAEoCRIYChBydW5uaW5nX3NpbmNlX21zGAsgASgEEhIKCmNvbXBhY3RpbmcYDCABKAgSDQoFY29sb3IYDSABKAkSDgoGdW5yZWFkGA4gASgIEioKBWxpbWl0GA8gASgLMhsua2V0LnJlbW90ZS52MS5TZXNzaW9uTGltaXQSIAoEam9icxgQIAMoCzISLmtldC5yZW1vdGUudjEuSm9iIoYBCgNKb2ISCwoDcGlkGAEgASgFEhUKDXN0YXJ0ZWRfYXRfbXMYAiABKAQSDwoHY29tbWFuZBgDIAEoCRITCgtkZXNjcmlwdGlvbhgEIAEoCRINCgVwb3J0cxgFIAMoDRImCgVlbmRlZBgGIAEoCzIXLmtldC5yZW1vdGUudjEuSm9iRW5kZWQivQEKCEpvYkVuZGVkEg0KBWF0X21zGAEgASgEEjAKB291dGNvbWUYAiABKA4yHy5rZXQucmVtb3RlLnYxLkpvYkVuZGVkLk91dGNvbWUSEQoEZXhpdBgDIAEoEUgAiAEBIlQKB091dGNvbWUSFwoTT1VUQ09NRV9VTlNQRUNJRklFRBAAEg0KCUNPTVBMRVRFRBABEgoKBkZBSUxFRBACEgsKB1NUT1BQRUQQAxIICgRHT05FEARCBwoFX2V4aXQiQgoHU3RvcEpvYhITCgt0ZXJtaW5hbF9pZBgBIAEoBBILCgNwaWQYAiABKAUSFQoNc3RhcnRlZF9hdF9tcxgDIAEoBCIfCghNYXJrU2VlbhITCgt0ZXJtaW5hbF9pZBgBIAEoBCIxCgpNYXJrVW5yZWFkEhMKC3dvcmt0cmVlX2lkGAEgASgJEg4KBnVucmVhZBgCIAEoCCKMAQoMUmVnaXN0ZXJQdXNoEg0KBXRva2VuGAEgASgMEjMKC2Vudmlyb25tZW50GAIgASgOMh4ua2V0LnJlbW90ZS52MS5QdXNoRW52aXJvbm1lbnQSEAoIcHVzaF9rZXkYAyABKAwSJgoFa2luZHMYBCADKA4yFy5rZXQucmVtb3RlLnYxLlB1c2hLaW5kIhAKDlVucmVnaXN0ZXJQdXNoImQKCUNvbGRDYWNoZRIPCgdzZXNzaW9uGAEgASgJEhAKCHNpbmNlX21zGAIgASgEEg4KBnRva2VucxgDIAEoBBITCgtjb3N0X21pY3JvcxgEIAEoAxIPCgdzdW1tYXJ5GAUgASgJIkQKClN0YXJ0RnJlc2gSEwoLdGVybWluYWxfaWQYASABKAQSDwoHc2Vzc2lvbhgCIAEoCRIQCghzaW5jZV9tcxgDIAEoBCIjCgxGcmVzaFN0YXJ0ZWQSEwoLdGVybWluYWxfaWQYASABKAQiXgoNUHJldmlld1N3aXRjaBITCgt0ZXJtaW5hbF9pZBgBIAEoBBINCgVhZ2VudBgCIAEoCRIpCgVkZXB0aBgDIAEoDjIaLmtldC5yZW1vdGUudjEuU3dpdGNoRGVwdGgiaQoNU3dpdGNoUHJldmlldxITCgt0ZXJtaW5hbF9pZBgBIAEoBBIMCgR0ZXh0GAIgASgJEg0KBXR1cm5zGAMgASgNEhUKDWFwcHJveF90b2tlbnMYBCABKA0SDwoHc2Vzc2lvbhgFIAEoCSJ/CgtTd2l0Y2hBZ2VudBITCgt0ZXJtaW5hbF9pZBgBIAEoBBINCgVhZ2VudBgCIAEoCRINCgVtb2RlbBgDIAEoCRIOCgZlZmZvcnQYBCABKAkSDAoEdGV4dBgFIAEoCRIOCgZiZXNpZGUYBiABKAgSDwoHc2Vzc2lvbhgHIAEoCSIkCg1BZ2VudFN3aXRjaGVkEhMKC3Rlcm1pbmFsX2lkGAEgASgEIkgKCkFnZW50TW9kZXMSDwoHY3VycmVudBgBIAEoCRIpCgdjaG9pY2VzGAIgAygLMhgua2V0LnJlbW90ZS52MS5BZ2VudE1vZGUiNgoJQWdlbnRNb2RlEgoKAmlkGAEgASgJEg0KBWxhYmVsGAIgASgJEg4KBmRldGFpbBgDIAEoCSIsCgdTZXRNb2RlEhMKC3Rlcm1pbmFsX2lkGAEgASgEEgwKBG1vZGUYAiABKAkingEKDUFnZW50U2V0dGluZ3MSDQoFbW9kZWwYASABKAkSDgoGZWZmb3J0GAIgASgJEioKBm1vZGVscxgDIAMoCzIaLmtldC5yZW1vdGUudjEuQWdlbnRDaG9pY2USKwoHZWZmb3J0cxgEIAMoCzIaLmtldC5yZW1vdGUudjEuQWdlbnRDaG9pY2USFQoNc2F2ZXNfZGVmYXVsdBgFIAEoCCIoCgtBZ2VudENob2ljZRIKCgJpZBgBIAEoCRINCgVsYWJlbBgCIAEoCSI+CghTZXRNb2RlbBITCgt0ZXJtaW5hbF9pZBgBIAEoBBINCgVtb2RlbBgCIAEoCRIOCgZlZmZvcnQYAyABKAkiTwoJQWdlbnRWaWV3EjIKCHNlc3Npb25zGAEgAygLMiAua2V0LnJlbW90ZS52MS5CYWNrZ3JvdW5kU2Vzc2lvbhIOCgZsaXN0ZWQYAiABKAgi6wEKEUJhY2tncm91bmRTZXNzaW9uEg8KB3Nlc3Npb24YASABKAkSDAoEbmFtZRgCIAEoCRI1CgVzdGF0ZRgDIAEoDjImLmtldC5yZW1vdGUudjEuQmFja2dyb3VuZFNlc3Npb24uU3RhdGUSFQoNc3RhcnRlZF9hdF9tcxgEIAEoAxIUCgxhY3RpdmVfYXRfbXMYBSABKAMSDQoFZG9pbmcYBiABKAkiRAoFU3RhdGUSFQoRU1RBVEVfVU5TUEVDSUZJRUQQABIICgRET05FEAESCwoHV09SS0lORxACEg0KCU5FRURTX1lPVRADIiAKCVN1YnNjcmliZRITCgt0ZXJtaW5hbF9pZBgBIAEoBCIiCgtVbnN1YnNjcmliZRITCgt0ZXJtaW5hbF9pZBgBIAEoBCKLAQoSVGVybWluYWxDaGVja3BvaW50EhMKC3Rlcm1pbmFsX2lkGAEgASgEEg0KBWVwb2NoGAIgASgEEhMKC3Rocm91Z2hfc2VxGAMgASgEEgwKBGNvbHMYBCABKA0SDAoEcm93cxgFIAEoDRISCgpzY3JvbGxiYWNrGAYgASgNEgwKBGFuc2kYByABKAwiTwoNVGVybWluYWxDaHVuaxITCgt0ZXJtaW5hbF9pZBgBIAEoBBINCgVlcG9jaBgCIAEoBBILCgNzZXEYAyABKAQSDQoFYnl0ZXMYBCABKAwiJAoNVGVybWluYWxSZXNldBITCgt0ZXJtaW5hbF9pZBgBIAEoBCJbCg5UZXJtaW5hbENsb3NlZBITCgt0ZXJtaW5hbF9pZBgBIAEoBBIWCglleGl0X2NvZGUYAiABKA1IAIgBARIOCgZzaWduYWwYAyABKAlCDAoKX2V4aXRfY29kZSIrCgVJbnB1dBITCgt0ZXJtaW5hbF9pZBgBIAEoBBINCgVieXRlcxgCIAEoDCJeCglTZW5kRmlsZXMSEwoLdGVybWluYWxfaWQYASABKAQSDAoEdGV4dBgCIAEoCRIuCgthdHRhY2htZW50cxgDIAMoCzIZLmtldC5yZW1vdGUudjEuQXR0YWNobWVudCIoCgpBdHRhY2htZW50EgwKBG5hbWUYASABKAkSDAoEZGF0YRgCIAEoDCI5CgZSZXNpemUSEwoLdGVybWluYWxfaWQYASABKAQSDAoEY29scxgCIAEoDRIMCgRyb3dzGAMgASgNIpABCgVMZWFzZRITCgt0ZXJtaW5hbF9pZBgBIAEoBBIrCgZhY3Rpb24YAiABKA4yGy5rZXQucmVtb3RlLnYxLkxlYXNlLkFjdGlvbiJFCgZBY3Rpb24SFgoSQUNUSU9OX1VOU1BFQ0lGSUVEEAASCwoHQUNRVUlSRRABEgkKBVJFTkVXEAISCwoHUkVMRUFTRRADIoMBCgZTaWduYWwSEwoLdGVybWluYWxfaWQYASABKAQSKAoEa2luZBgCIAEoDjIaLmtldC5yZW1vdGUudjEuU2lnbmFsLktpbmQiOgoES2luZBIUChBLSU5EX1VOU1BFQ0lGSUVEEAASDQoJSU5URVJSVVBUEAESDQoJVEVSTUlOQVRFEAIirgEKBkFuc3dlchITCgt0ZXJtaW5hbF9pZBgBIAEoBBIsCgZjaG9pY2UYAiABKA4yHC5rZXQucmVtb3RlLnYxLkFuc3dlci5DaG9pY2USEwoLcXVlc3Rpb25faWQYAyABKAQiTAoGQ2hvaWNlEhYKEkNIT0lDRV9VTlNQRUNJRklFRBAAEg4KCkFMTE9XX09OQ0UQARIQCgxBTExPV19BTFdBWVMQAhIICgRERU5ZEAMiqgEKB1Jlc3BvbmQSEwoLdGVybWluYWxfaWQYASABKAQSEwoLcXVlc3Rpb25faWQYAiABKAQSKgoFcGlja3MYAyADKAsyGy5rZXQucmVtb3RlLnYxLlJlc3BvbmQuUGljaxIPCgdhcHByb3ZlGAQgASgIEhAKCGZlZWRiYWNrGAUgASgJGiYKBFBpY2sSDwoHY2hvaWNlcxgBIAMoDRINCgVvdGhlchgCIAEoCSJGCg9HZXRDb252ZXJzYXRpb24SEwoLdGVybWluYWxfaWQYASABKAQSDQoFYWZ0ZXIYAiABKAQSDwoHc2Vzc2lvbhgDIAEoCSK/AQoMQ29udmVyc2F0aW9uEhMKC3Rlcm1pbmFsX2lkGAEgASgEEhEKCXN1cHBvcnRlZBgCIAEoCBIOCgZyZWFzb24YAyABKAkSDwoHc2Vzc2lvbhgEIAEoCRIiCgV0dXJucxgFIAMoCzITLmtldC5yZW1vdGUudjEuVHVybhIMCgRuZXh0GAYgASgEEg0KBWZyZXNoGAcgASgIEg4KBnF1ZXVlZBgIIAMoCRIVCg1zdGFydGVkX2F0X21zGAkgASgEIpsBCgRUdXJuEiYKBGtpbmQYASABKA4yGC5rZXQucmVtb3RlLnYxLlR1cm4uS2luZBIMCgR0ZXh0GAIgASgJEgwKBHRvb2wYAyABKAkSDgoGaW1hZ2VzGAQgASgNIj8KBEtpbmQSFAoQS0lORF9VTlNQRUNJRklFRBAAEggKBFVTRVIQARINCglBU1NJU1RBTlQQAhIICgRUT09MEAMiawoJU3RhcnRXb3JrEhIKCnByb2plY3RfaWQYASABKAkSDgoGcHJvbXB0GAIgASgJEg0KBWFnZW50GAMgASgJEgwKBGJhc2UYBCABKAkSDQoFbW9kZWwYBSABKAkSDgoGZWZmb3J0GAYgASgJIiUKD0dldFN0YXJ0T3B0aW9ucxISCgpwcm9qZWN0X2lkGAEgASgJIkMKEUFnZW50U3RhcnRPcHRpb25zEg0KBWFnZW50GAEgASgJEg4KBm1vZGVscxgCIAMoCRIPCgdlZmZvcnRzGAMgAygJIoIBCgxTdGFydE9wdGlvbnMSEgoKcHJvamVjdF9pZBgBIAEoCRIUCgxkZWZhdWx0X2Jhc2UYAiABKAkSFgoObG9jYWxfYnJhbmNoZXMYAyADKAkSMAoGYWdlbnRzGAQgAygLMiAua2V0LnJlbW90ZS52MS5BZ2VudFN0YXJ0T3B0aW9ucyJhCgxTdGFydFNlc3Npb24SEwoLd29ya3RyZWVfaWQYASABKAkSDQoFYWdlbnQYAiABKAkSDgoGcHJvbXB0GAMgASgJEg0KBW1vZGVsGAQgASgJEg4KBmVmZm9ydBgFIAEoCSIgCgpHZXRCYWNrbG9nEhIKCnByb2plY3RfaWQYASABKAkiSAoHQmFja2xvZxISCgpwcm9qZWN0X2lkGAEgASgJEikKBW5vdGVzGAIgAygLMhoua2V0LnJlbW90ZS52MS5CYWNrbG9nTm90ZSLAAQoLQmFja2xvZ05vdGUSCgoCaWQYASABKAkSDQoFdGl0bGUYAiABKAkSDAoEYm9keRgDIAEoCRINCgVmaWxlcxgEIAMoCRISCgpjcmVhdGVkX21zGAUgASgEEhIKCnVwZGF0ZWRfbXMYBiABKAQSDwoHZG9uZV9tcxgHIAEoBBIOCgZicmFuY2gYCCABKAkSMAoIcHJpb3JpdHkYCSABKA4yHi5rZXQucmVtb3RlLnYxLkJhY2tsb2dQcmlvcml0eSKCAgoLRWRpdEJhY2tsb2cSEgoKcHJvamVjdF9pZBgBIAEoCRIPCgdub3RlX2lkGAIgASgJEjEKBmFjdGlvbhgDIAEoDjIhLmtldC5yZW1vdGUudjEuRWRpdEJhY2tsb2cuQWN0aW9uEg0KBXRpdGxlGAQgASgJEgwKBGJvZHkYBSABKAkSMAoIcHJpb3JpdHkYBiABKA4yHi5rZXQucmVtb3RlLnYxLkJhY2tsb2dQcmlvcml0eSJMCgZBY3Rpb24SFgoSQUNUSU9OX1VOU1BFQ0lGSUVEEAASCAoEU0FWRRABEggKBERPTkUQAhIKCgZSRU9QRU4QAxIKCgZSRU1PVkUQBCIwCglTdGFydE5vdGUSEgoKcHJvamVjdF9pZBgBIAEoCRIPCgdub3RlX2lkGAIgASgJIiEKCkdldENoYW5nZXMSEwoLd29ya3RyZWVfaWQYASABKAkiZQoHQ2hhbmdlcxITCgt3b3JrdHJlZV9pZBgBIAEoCRIoCgVmaWxlcxgCIAMoCzIZLmtldC5yZW1vdGUudjEuRmlsZUNoYW5nZRIMCgRiYXNlGAMgASgJEg0KBWVycm9yGAQgASgJIsQBCgpGaWxlQ2hhbmdlEgwKBHBhdGgYASABKAkSDQoFYWRkZWQYAiABKA0SDwoHcmVtb3ZlZBgDIAEoDRIwCgZzdGF0dXMYBCABKA4yIC5rZXQucmVtb3RlLnYxLkZpbGVDaGFuZ2UuU3RhdHVzEg4KBmJpbmFyeRgFIAEoCCJGCgZTdGF0dXMSFgoSU1RBVFVTX1VOU1BFQ0lGSUVEEAASDAoITU9ESUZJRUQQARIJCgVBRERFRBACEgsKB0RFTEVURUQQAyIsCgdHZXREaWZmEhMKC3dvcmt0cmVlX2lkGAEgASgJEgwKBHBhdGgYAiABKAkiWQoERGlmZhITCgt3b3JrdHJlZV9pZBgBIAEoCRIMCgRwYXRoGAIgASgJEgwKBHRleHQYAyABKAkSEQoJdHJ1bmNhdGVkGAQgASgIEg0KBWVycm9yGAUgASgJIiQKDU1lcmdlV29ya3RyZWUSEwoLd29ya3RyZWVfaWQYASABKAkiIAoJRml4Q2hlY2tzEhMKC3dvcmt0cmVlX2lkGAEgASgJIiQKEENoZWNrc0hhbmRlZE92ZXISEAoIc3RhcnRpbmcYASABKAgiUQoKU2VuZFJldmlldxITCgt3b3JrdHJlZV9pZBgBIAEoCRIuCghjb21tZW50cxgCIAMoCzIcLmtldC5yZW1vdGUudjEuUmV2aWV3Q29tbWVudCJLCg1SZXZpZXdDb21tZW50EgwKBHBhdGgYASABKAkSDQoFbGluZXMYAiABKAkSDAoEdGV4dBgDIAEoCRIPCgdjb21tZW50GAQgASgJIioKE0dldFB1bGxSZXF1ZXN0RHJhZnQSEwoLd29ya3RyZWVfaWQYASABKAkisgEKEFB1bGxSZXF1ZXN0RHJhZnQSEwoLd29ya3RyZWVfaWQYASABKAkSDgoGYnJhbmNoGAIgASgJEgwKBGJhc2UYAyABKAkSDgoGcmVtb3RlGAQgASgJEg0KBXRpdGxlGAUgASgJEgwKBGJvZHkYBiABKAkSDwoHY2hhbmdlcxgHIAEoDRIXCg9leGlzdGluZ19udW1iZXIYCCABKAQSFAoMZXhpc3RpbmdfdXJsGAkgASgJIlIKD09wZW5QdWxsUmVxdWVzdBITCgt3b3JrdHJlZV9pZBgBIAEoCRINCgV0aXRsZRgCIAEoCRIMCgRib2R5GAMgASgJEg0KBWRyYWZ0GAQgASgIIoIBChFQdWxsUmVxdWVzdE9wZW5lZBIOCgZudW1iZXIYASABKAQSCwoDdXJsGAIgASgJEg4KBmJyYW5jaBgDIAEoCRIMCgRiYXNlGAQgASgJEg4KBnJlbW90ZRgFIAEoCRIPCgdleGlzdGVkGAYgASgIEhEKCWNvbW1pdHRlZBgHIAEoDSJeChBNZXJnZVB1bGxSZXF1ZXN0EhMKC3dvcmt0cmVlX2lkGAEgASgJEjUKBm1ldGhvZBgCIAEoDjIlLmtldC5yZW1vdGUudjEuUHVsbFJlcXVlc3RNZXJnZU1ldGhvZCJnChFQdWxsUmVxdWVzdE1lcmdlZBIOCgZudW1iZXIYASABKAQSCwoDdXJsGAIgASgJEjUKBm1ldGhvZBgDIAEoDjIlLmtldC5yZW1vdGUudjEuUHVsbFJlcXVlc3RNZXJnZU1ldGhvZCIoChFVcGRhdGVQdWxsUmVxdWVzdBITCgt3b3JrdHJlZV9pZBgBIAEoCSIkChJQdWxsUmVxdWVzdFVwZGF0ZWQSDgoGbnVtYmVyGAEgASgEIicKEFJlYWR5UHVsbFJlcXVlc3QSEwoLd29ya3RyZWVfaWQYASABKAkiJAoSUHVsbFJlcXVlc3RSZWFkaWVkEg4KBm51bWJlchgBIAEoBCIoChFSZW9wZW5QdWxsUmVxdWVzdBITCgt3b3JrdHJlZV9pZBgBIAEoCSIlChNQdWxsUmVxdWVzdFJlb3BlbmVkEg4KBm51bWJlchgBIAEoBCImCg9BZGRyZXNzQ29tbWVudHMSEwoLd29ya3RyZWVfaWQYASABKAkiJgoSQ29tbWVudHNIYW5kZWRPdmVyEhAKCHN0YXJ0aW5nGAEgASgIIkYKEE9wZW5BZ2VudFNlc3Npb24SEwoLdGVybWluYWxfaWQYASABKAQSDwoHc2Vzc2lvbhgCIAEoCRIMCgR0ZXh0GAMgASgJIjIKC1Bpbldvcmt0cmVlEhMKC3dvcmt0cmVlX2lkGAEgASgJEg4KBnBpbm5lZBgCIAEoCCIzCg5SZW5hbWVXb3JrdHJlZRITCgt3b3JrdHJlZV9pZBgBIAEoCRIMCgRuYW1lGAIgASgJIjQKDkRlbGV0ZVdvcmt0cmVlEhMKC3dvcmt0cmVlX2lkGAEgASgJEg0KBWZvcmNlGAIgASgIIiYKD1dvcmt0cmVlRGVsZXRlZBITCgtrZXB0X2JyYW5jaBgBIAEoCSINCgtHZXRTbmlwcGV0cyI3CgtTbmlwcGV0TGlzdBIoCghzbmlwcGV0cxgBIAMoCzIWLmtldC5yZW1vdGUudjEuU25pcHBldCIlCgdTbmlwcGV0EgwKBG5hbWUYASABKAkSDAoEYm9keRgCIAEoCSINCgtMaXN0RGV2aWNlcyI0CgpEZXZpY2VMaXN0EiYKB2RldmljZXMYASADKAsyFS5rZXQucmVtb3RlLnYxLkRldmljZSJrCgZEZXZpY2USCgoCaWQYASABKAkSDAoEbmFtZRgCIAEoCRIRCglwYWlyZWRfYXQYAyABKAQSEwoLdGhpc19kZXZpY2UYBCABKAgSEQoJY29ubmVjdGVkGAUgASgIEgwKBHB1c2gYBiABKAgiGgoMUmV2b2tlRGV2aWNlEgoKAmlkGAEgASgJIhAKDkdldEF1dG9tYXRpb25zIkQKC0F1dG9tYXRpb25zEjUKC2F1dG9tYXRpb25zGAEgAygLMiAua2V0LnJlbW90ZS52MS5BdXRvbWF0aW9uU3VtbWFyeSLGAQoRQXV0b21hdGlvblN1bW1hcnkSCgoCaWQYASABKAkSDAoEbmFtZRgCIAEoCRISCgpwcm9qZWN0X2lkGAMgASgJEhAKCHNjaGVkdWxlGAQgASgJEg8KB2VuYWJsZWQYBSABKAgSEAoIZmluaXNoZWQYBiABKAgSFQoNbmV4dF9zdGFydF9tcxgHIAEoBBITCgtsYXN0X3Jlc3VsdBgIIAEoCRITCgtsYXN0X3J1bl9tcxgJIAEoBBINCgVhZ2VudBgKIAEoCSJUCg5FZGl0QXV0b21hdGlvbhIVCg1hdXRvbWF0aW9uX2lkGAEgASgJEisKBGVkaXQYAiABKA4yHS5rZXQucmVtb3RlLnYxLkF1dG9tYXRpb25FZGl0KssBCglFcnJvckNvZGUSGgoWRVJST1JfQ09ERV9VTlNQRUNJRklFRBAAEhAKDFVOQVVUSE9SSVpFRBABEhEKDVNUQUxFX1JFUVVFU1QQAhINCglOT1RfRk9VTkQQAxISCg5OT1RfQ09OVFJPTExFUhAEEhAKDEJBQ0tQUkVTU1VSRRAFEhcKE1VOU1VQUE9SVEVEX1ZFUlNJT04QBhIMCghERUNMSU5FRBAHEhMKD1dPVUxEX0xPU0VfV09SSxAIEgwKCEJMQUNLT1VUEAkqlwEKC1VzYWdlU3RhdHVzEhwKGFVTQUdFX1NUQVRVU19VTlNQRUNJRklFRBAAEhYKElVTQUdFX1NUQVRVU19GUkVTSBABEhYKElVTQUdFX1NUQVRVU19TVEFMRRACEhwKGFVTQUdFX1NUQVRVU19VTkFWQUlMQUJMRRADEhwKGFVTQUdFX1NUQVRVU19VTlNVUFBPUlRFRBAEKosBCghBY3Rpdml0eRISCg5BQ1RJVklUWV9RVUlFVBAAEhQKEEFDVElWSVRZX1JVTk5JTkcQARIUChBBQ1RJVklUWV9XT1JLSU5HEAISFAoQQUNUSVZJVFlfTUVSR0lORxADEhQKEEFDVElWSVRZX0JMT0NLRUQQBBITCg9BQ1RJVklUWV9GQUlMRUQQBSpyCg9QdXNoRW52aXJvbm1lbnQSIAocUFVTSF9FTlZJUk9OTUVOVF9VTlNQRUNJRklFRBAAEhwKGFBVU0hfRU5WSVJPTk1FTlRfU0FOREJPWBABEh8KG1BVU0hfRU5WSVJPTk1FTlRfUFJPRFVDVElPThACKoABCghQdXNoS2luZBIZChVQVVNIX0tJTkRfVU5TUEVDSUZJRUQQABIVChFQVVNIX0tJTkRfV0FJVElORxABEhYKElBVU0hfS0lORF9GSU5JU0hFRBACEhQKEFBVU0hfS0lORF9DSEVDS1MQAxIUChBQVVNIX0tJTkRfTElNSVRTEAQqcgoLU3dpdGNoRGVwdGgSHAoYU1dJVENIX0RFUFRIX1VOU1BFQ0lGSUVEEAASFQoRU1dJVENIX0RFUFRIX1RBU0sQARIXChNTV0lUQ0hfREVQVEhfUkVDRU5UEAISFQoRU1dJVENIX0RFUFRIX0ZVTEwQAyqiAQoPQmFja2xvZ1ByaW9yaXR5EiAKHEJBQ0tMT0dfUFJJT1JJVFlfVU5TUEVDSUZJRUQQABIYChRCQUNLTE9HX1BSSU9SSVRZX0xPVxABEhsKF0JBQ0tMT0dfUFJJT1JJVFlfTUVESVVNEAISGQoVQkFDS0xPR19QUklPUklUWV9ISUdIEAMSGwoXQkFDS0xPR19QUklPUklUWV9VUkdFTlQQBCptChZQdWxsUmVxdWVzdE1lcmdlTWV0aG9kEikKJVBVTExfUkVRVUVTVF9NRVJHRV9NRVRIT0RfVU5TUEVDSUZJRUQQABIQCgxNRVJHRV9DT01NSVQQARIKCgZTUVVBU0gQAhIKCgZSRUJBU0UQAyqhAQoOQXV0b21hdGlvbkVkaXQSHwobQVVUT01BVElPTl9FRElUX1VOU1BFQ0lGSUVEEAASGQoVQVVUT01BVElPTl9FRElUX1BBVVNFEAESGgoWQVVUT01BVElPTl9FRElUX1JFU1VNRRACEhsKF0FVVE9NQVRJT05fRURJVF9SVU5fTk9XEAMSGgoWQVVUT01BVElPTl9FRElUX0RFTEVURRAEYgZwcm90bzM");

/**
 * Describes the message ket.remote.v1.Envelope.
 * Use `create(EnvelopeSchema)` to create a new message.
 */
export const EnvelopeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 0);

/**
 * Describes the message ket.remote.v1.Hello.
 * Use `create(HelloSchema)` to create a new message.
 */
export const HelloSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 1);

/**
 * Describes the message ket.remote.v1.PairingPending.
 * Use `create(PairingPendingSchema)` to create a new message.
 */
export const PairingPendingSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 2);

/**
 * Describes the message ket.remote.v1.Welcome.
 * Use `create(WelcomeSchema)` to create a new message.
 */
export const WelcomeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 3);

/**
 * Describes the message ket.remote.v1.Error.
 * Use `create(ErrorSchema)` to create a new message.
 */
export const ErrorSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 4);

/**
 * Describes the message ket.remote.v1.Ack.
 * Use `create(AckSchema)` to create a new message.
 */
export const AckSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 5);

/**
 * Describes the message ket.remote.v1.HostSnapshot.
 * Use `create(HostSnapshotSchema)` to create a new message.
 */
export const HostSnapshotSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 6);

/**
 * Describes the message ket.remote.v1.Blackout.
 * Use `create(BlackoutSchema)` to create a new message.
 */
export const BlackoutSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 7);

/**
 * Describes the message ket.remote.v1.BlackoutWindow.
 * Use `create(BlackoutWindowSchema)` to create a new message.
 */
export const BlackoutWindowSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 8);

/**
 * Describes the message ket.remote.v1.Palette.
 * Use `create(PaletteSchema)` to create a new message.
 */
export const PaletteSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 9);

/**
 * Describes the message ket.remote.v1.ProviderUsage.
 * Use `create(ProviderUsageSchema)` to create a new message.
 */
export const ProviderUsageSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 10);

/**
 * Describes the message ket.remote.v1.UsageWindow.
 * Use `create(UsageWindowSchema)` to create a new message.
 */
export const UsageWindowSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 11);

/**
 * Describes the message ket.remote.v1.ProjectSummary.
 * Use `create(ProjectSummarySchema)` to create a new message.
 */
export const ProjectSummarySchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 12);

/**
 * Describes the message ket.remote.v1.WorktreeSummary.
 * Use `create(WorktreeSummarySchema)` to create a new message.
 */
export const WorktreeSummarySchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 13);

/**
 * Describes the message ket.remote.v1.PullRequestSummary.
 * Use `create(PullRequestSummarySchema)` to create a new message.
 */
export const PullRequestSummarySchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 14);

/**
 * Describes the enum ket.remote.v1.PullRequestSummary.Review.
 */
export const PullRequestSummary_ReviewSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 14, 0);

/**
 * @generated from enum ket.remote.v1.PullRequestSummary.Review
 */
export const PullRequestSummary_Review = /*@__PURE__*/
  tsEnum(PullRequestSummary_ReviewSchema);

/**
 * Describes the enum ket.remote.v1.PullRequestSummary.State.
 */
export const PullRequestSummary_StateSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 14, 1);

/**
 * Since 1.19. Missing on an older desktop, which only sent open ones.
 *
 * @generated from enum ket.remote.v1.PullRequestSummary.State
 */
export const PullRequestSummary_State = /*@__PURE__*/
  tsEnum(PullRequestSummary_StateSchema);

/**
 * Describes the message ket.remote.v1.CheckSummary.
 * Use `create(CheckSummarySchema)` to create a new message.
 */
export const CheckSummarySchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 15);

/**
 * Describes the enum ket.remote.v1.CheckSummary.State.
 */
export const CheckSummary_StateSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 15, 0);

/**
 * @generated from enum ket.remote.v1.CheckSummary.State
 */
export const CheckSummary_State = /*@__PURE__*/
  tsEnum(CheckSummary_StateSchema);

/**
 * Describes the message ket.remote.v1.SubagentSummary.
 * Use `create(SubagentSummarySchema)` to create a new message.
 */
export const SubagentSummarySchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 16);

/**
 * Describes the enum ket.remote.v1.SubagentSummary.State.
 */
export const SubagentSummary_StateSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 16, 0);

/**
 * @generated from enum ket.remote.v1.SubagentSummary.State
 */
export const SubagentSummary_State = /*@__PURE__*/
  tsEnum(SubagentSummary_StateSchema);

/**
 * Describes the message ket.remote.v1.Question.
 * Use `create(QuestionSchema)` to create a new message.
 */
export const QuestionSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 17);

/**
 * Describes the message ket.remote.v1.Asked.
 * Use `create(AskedSchema)` to create a new message.
 */
export const AskedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 18);

/**
 * Describes the message ket.remote.v1.Asked.Choice.
 * Use `create(Asked_ChoiceSchema)` to create a new message.
 */
export const Asked_ChoiceSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 18, 0);

/**
 * Describes the message ket.remote.v1.Risk.
 * Use `create(RiskSchema)` to create a new message.
 */
export const RiskSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 19);

/**
 * Describes the message ket.remote.v1.AsleepTerminal.
 * Use `create(AsleepTerminalSchema)` to create a new message.
 */
export const AsleepTerminalSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 20);

/**
 * Describes the message ket.remote.v1.SessionLimit.
 * Use `create(SessionLimitSchema)` to create a new message.
 */
export const SessionLimitSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 21);

/**
 * Describes the enum ket.remote.v1.SessionLimit.Scope.
 */
export const SessionLimit_ScopeSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 21, 0);

/**
 * @generated from enum ket.remote.v1.SessionLimit.Scope
 */
export const SessionLimit_Scope = /*@__PURE__*/
  tsEnum(SessionLimit_ScopeSchema);

/**
 * Describes the enum ket.remote.v1.SessionLimit.Window.
 */
export const SessionLimit_WindowSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 21, 1);

/**
 * @generated from enum ket.remote.v1.SessionLimit.Window
 */
export const SessionLimit_Window = /*@__PURE__*/
  tsEnum(SessionLimit_WindowSchema);

/**
 * Describes the message ket.remote.v1.WakeTerminal.
 * Use `create(WakeTerminalSchema)` to create a new message.
 */
export const WakeTerminalSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 22);

/**
 * Describes the message ket.remote.v1.TerminalWoken.
 * Use `create(TerminalWokenSchema)` to create a new message.
 */
export const TerminalWokenSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 23);

/**
 * Describes the message ket.remote.v1.TerminalSummary.
 * Use `create(TerminalSummarySchema)` to create a new message.
 */
export const TerminalSummarySchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 24);

/**
 * Describes the message ket.remote.v1.Job.
 * Use `create(JobSchema)` to create a new message.
 */
export const JobSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 25);

/**
 * Describes the message ket.remote.v1.JobEnded.
 * Use `create(JobEndedSchema)` to create a new message.
 */
export const JobEndedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 26);

/**
 * Describes the enum ket.remote.v1.JobEnded.Outcome.
 */
export const JobEnded_OutcomeSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 26, 0);

/**
 * @generated from enum ket.remote.v1.JobEnded.Outcome
 */
export const JobEnded_Outcome = /*@__PURE__*/
  tsEnum(JobEnded_OutcomeSchema);

/**
 * Describes the message ket.remote.v1.StopJob.
 * Use `create(StopJobSchema)` to create a new message.
 */
export const StopJobSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 27);

/**
 * Describes the message ket.remote.v1.MarkSeen.
 * Use `create(MarkSeenSchema)` to create a new message.
 */
export const MarkSeenSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 28);

/**
 * Describes the message ket.remote.v1.MarkUnread.
 * Use `create(MarkUnreadSchema)` to create a new message.
 */
export const MarkUnreadSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 29);

/**
 * Describes the message ket.remote.v1.RegisterPush.
 * Use `create(RegisterPushSchema)` to create a new message.
 */
export const RegisterPushSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 30);

/**
 * Describes the message ket.remote.v1.UnregisterPush.
 * Use `create(UnregisterPushSchema)` to create a new message.
 */
export const UnregisterPushSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 31);

/**
 * Describes the message ket.remote.v1.ColdCache.
 * Use `create(ColdCacheSchema)` to create a new message.
 */
export const ColdCacheSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 32);

/**
 * Describes the message ket.remote.v1.StartFresh.
 * Use `create(StartFreshSchema)` to create a new message.
 */
export const StartFreshSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 33);

/**
 * Describes the message ket.remote.v1.FreshStarted.
 * Use `create(FreshStartedSchema)` to create a new message.
 */
export const FreshStartedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 34);

/**
 * Describes the message ket.remote.v1.PreviewSwitch.
 * Use `create(PreviewSwitchSchema)` to create a new message.
 */
export const PreviewSwitchSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 35);

/**
 * Describes the message ket.remote.v1.SwitchPreview.
 * Use `create(SwitchPreviewSchema)` to create a new message.
 */
export const SwitchPreviewSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 36);

/**
 * Describes the message ket.remote.v1.SwitchAgent.
 * Use `create(SwitchAgentSchema)` to create a new message.
 */
export const SwitchAgentSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 37);

/**
 * Describes the message ket.remote.v1.AgentSwitched.
 * Use `create(AgentSwitchedSchema)` to create a new message.
 */
export const AgentSwitchedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 38);

/**
 * Describes the message ket.remote.v1.AgentModes.
 * Use `create(AgentModesSchema)` to create a new message.
 */
export const AgentModesSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 39);

/**
 * Describes the message ket.remote.v1.AgentMode.
 * Use `create(AgentModeSchema)` to create a new message.
 */
export const AgentModeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 40);

/**
 * Describes the message ket.remote.v1.SetMode.
 * Use `create(SetModeSchema)` to create a new message.
 */
export const SetModeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 41);

/**
 * Describes the message ket.remote.v1.AgentSettings.
 * Use `create(AgentSettingsSchema)` to create a new message.
 */
export const AgentSettingsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 42);

/**
 * Describes the message ket.remote.v1.AgentChoice.
 * Use `create(AgentChoiceSchema)` to create a new message.
 */
export const AgentChoiceSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 43);

/**
 * Describes the message ket.remote.v1.SetModel.
 * Use `create(SetModelSchema)` to create a new message.
 */
export const SetModelSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 44);

/**
 * Describes the message ket.remote.v1.AgentView.
 * Use `create(AgentViewSchema)` to create a new message.
 */
export const AgentViewSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 45);

/**
 * Describes the message ket.remote.v1.BackgroundSession.
 * Use `create(BackgroundSessionSchema)` to create a new message.
 */
export const BackgroundSessionSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 46);

/**
 * Describes the enum ket.remote.v1.BackgroundSession.State.
 */
export const BackgroundSession_StateSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 46, 0);

/**
 * @generated from enum ket.remote.v1.BackgroundSession.State
 */
export const BackgroundSession_State = /*@__PURE__*/
  tsEnum(BackgroundSession_StateSchema);

/**
 * Describes the message ket.remote.v1.Subscribe.
 * Use `create(SubscribeSchema)` to create a new message.
 */
export const SubscribeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 47);

/**
 * Describes the message ket.remote.v1.Unsubscribe.
 * Use `create(UnsubscribeSchema)` to create a new message.
 */
export const UnsubscribeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 48);

/**
 * Describes the message ket.remote.v1.TerminalCheckpoint.
 * Use `create(TerminalCheckpointSchema)` to create a new message.
 */
export const TerminalCheckpointSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 49);

/**
 * Describes the message ket.remote.v1.TerminalChunk.
 * Use `create(TerminalChunkSchema)` to create a new message.
 */
export const TerminalChunkSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 50);

/**
 * Describes the message ket.remote.v1.TerminalReset.
 * Use `create(TerminalResetSchema)` to create a new message.
 */
export const TerminalResetSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 51);

/**
 * Describes the message ket.remote.v1.TerminalClosed.
 * Use `create(TerminalClosedSchema)` to create a new message.
 */
export const TerminalClosedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 52);

/**
 * Describes the message ket.remote.v1.Input.
 * Use `create(InputSchema)` to create a new message.
 */
export const InputSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 53);

/**
 * Describes the message ket.remote.v1.SendFiles.
 * Use `create(SendFilesSchema)` to create a new message.
 */
export const SendFilesSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 54);

/**
 * Describes the message ket.remote.v1.Attachment.
 * Use `create(AttachmentSchema)` to create a new message.
 */
export const AttachmentSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 55);

/**
 * Describes the message ket.remote.v1.Resize.
 * Use `create(ResizeSchema)` to create a new message.
 */
export const ResizeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 56);

/**
 * Describes the message ket.remote.v1.Lease.
 * Use `create(LeaseSchema)` to create a new message.
 */
export const LeaseSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 57);

/**
 * Describes the enum ket.remote.v1.Lease.Action.
 */
export const Lease_ActionSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 57, 0);

/**
 * @generated from enum ket.remote.v1.Lease.Action
 */
export const Lease_Action = /*@__PURE__*/
  tsEnum(Lease_ActionSchema);

/**
 * Describes the message ket.remote.v1.Signal.
 * Use `create(SignalSchema)` to create a new message.
 */
export const SignalSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 58);

/**
 * Describes the enum ket.remote.v1.Signal.Kind.
 */
export const Signal_KindSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 58, 0);

/**
 * @generated from enum ket.remote.v1.Signal.Kind
 */
export const Signal_Kind = /*@__PURE__*/
  tsEnum(Signal_KindSchema);

/**
 * Describes the message ket.remote.v1.Answer.
 * Use `create(AnswerSchema)` to create a new message.
 */
export const AnswerSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 59);

/**
 * Describes the enum ket.remote.v1.Answer.Choice.
 */
export const Answer_ChoiceSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 59, 0);

/**
 * @generated from enum ket.remote.v1.Answer.Choice
 */
export const Answer_Choice = /*@__PURE__*/
  tsEnum(Answer_ChoiceSchema);

/**
 * Describes the message ket.remote.v1.Respond.
 * Use `create(RespondSchema)` to create a new message.
 */
export const RespondSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 60);

/**
 * Describes the message ket.remote.v1.Respond.Pick.
 * Use `create(Respond_PickSchema)` to create a new message.
 */
export const Respond_PickSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 60, 0);

/**
 * Describes the message ket.remote.v1.GetConversation.
 * Use `create(GetConversationSchema)` to create a new message.
 */
export const GetConversationSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 61);

/**
 * Describes the message ket.remote.v1.Conversation.
 * Use `create(ConversationSchema)` to create a new message.
 */
export const ConversationSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 62);

/**
 * Describes the message ket.remote.v1.Turn.
 * Use `create(TurnSchema)` to create a new message.
 */
export const TurnSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 63);

/**
 * Describes the enum ket.remote.v1.Turn.Kind.
 */
export const Turn_KindSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 63, 0);

/**
 * @generated from enum ket.remote.v1.Turn.Kind
 */
export const Turn_Kind = /*@__PURE__*/
  tsEnum(Turn_KindSchema);

/**
 * Describes the message ket.remote.v1.StartWork.
 * Use `create(StartWorkSchema)` to create a new message.
 */
export const StartWorkSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 64);

/**
 * Describes the message ket.remote.v1.GetStartOptions.
 * Use `create(GetStartOptionsSchema)` to create a new message.
 */
export const GetStartOptionsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 65);

/**
 * Describes the message ket.remote.v1.AgentStartOptions.
 * Use `create(AgentStartOptionsSchema)` to create a new message.
 */
export const AgentStartOptionsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 66);

/**
 * Describes the message ket.remote.v1.StartOptions.
 * Use `create(StartOptionsSchema)` to create a new message.
 */
export const StartOptionsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 67);

/**
 * Describes the message ket.remote.v1.StartSession.
 * Use `create(StartSessionSchema)` to create a new message.
 */
export const StartSessionSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 68);

/**
 * Describes the message ket.remote.v1.GetBacklog.
 * Use `create(GetBacklogSchema)` to create a new message.
 */
export const GetBacklogSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 69);

/**
 * Describes the message ket.remote.v1.Backlog.
 * Use `create(BacklogSchema)` to create a new message.
 */
export const BacklogSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 70);

/**
 * Describes the message ket.remote.v1.BacklogNote.
 * Use `create(BacklogNoteSchema)` to create a new message.
 */
export const BacklogNoteSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 71);

/**
 * Describes the message ket.remote.v1.EditBacklog.
 * Use `create(EditBacklogSchema)` to create a new message.
 */
export const EditBacklogSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 72);

/**
 * Describes the enum ket.remote.v1.EditBacklog.Action.
 */
export const EditBacklog_ActionSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 72, 0);

/**
 * @generated from enum ket.remote.v1.EditBacklog.Action
 */
export const EditBacklog_Action = /*@__PURE__*/
  tsEnum(EditBacklog_ActionSchema);

/**
 * Describes the message ket.remote.v1.StartNote.
 * Use `create(StartNoteSchema)` to create a new message.
 */
export const StartNoteSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 73);

/**
 * Describes the message ket.remote.v1.GetChanges.
 * Use `create(GetChangesSchema)` to create a new message.
 */
export const GetChangesSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 74);

/**
 * Describes the message ket.remote.v1.Changes.
 * Use `create(ChangesSchema)` to create a new message.
 */
export const ChangesSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 75);

/**
 * Describes the message ket.remote.v1.FileChange.
 * Use `create(FileChangeSchema)` to create a new message.
 */
export const FileChangeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 76);

/**
 * Describes the enum ket.remote.v1.FileChange.Status.
 */
export const FileChange_StatusSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 76, 0);

/**
 * @generated from enum ket.remote.v1.FileChange.Status
 */
export const FileChange_Status = /*@__PURE__*/
  tsEnum(FileChange_StatusSchema);

/**
 * Describes the message ket.remote.v1.GetDiff.
 * Use `create(GetDiffSchema)` to create a new message.
 */
export const GetDiffSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 77);

/**
 * Describes the message ket.remote.v1.Diff.
 * Use `create(DiffSchema)` to create a new message.
 */
export const DiffSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 78);

/**
 * Describes the message ket.remote.v1.MergeWorktree.
 * Use `create(MergeWorktreeSchema)` to create a new message.
 */
export const MergeWorktreeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 79);

/**
 * Describes the message ket.remote.v1.FixChecks.
 * Use `create(FixChecksSchema)` to create a new message.
 */
export const FixChecksSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 80);

/**
 * Describes the message ket.remote.v1.ChecksHandedOver.
 * Use `create(ChecksHandedOverSchema)` to create a new message.
 */
export const ChecksHandedOverSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 81);

/**
 * Describes the message ket.remote.v1.SendReview.
 * Use `create(SendReviewSchema)` to create a new message.
 */
export const SendReviewSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 82);

/**
 * Describes the message ket.remote.v1.ReviewComment.
 * Use `create(ReviewCommentSchema)` to create a new message.
 */
export const ReviewCommentSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 83);

/**
 * Describes the message ket.remote.v1.GetPullRequestDraft.
 * Use `create(GetPullRequestDraftSchema)` to create a new message.
 */
export const GetPullRequestDraftSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 84);

/**
 * Describes the message ket.remote.v1.PullRequestDraft.
 * Use `create(PullRequestDraftSchema)` to create a new message.
 */
export const PullRequestDraftSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 85);

/**
 * Describes the message ket.remote.v1.OpenPullRequest.
 * Use `create(OpenPullRequestSchema)` to create a new message.
 */
export const OpenPullRequestSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 86);

/**
 * Describes the message ket.remote.v1.PullRequestOpened.
 * Use `create(PullRequestOpenedSchema)` to create a new message.
 */
export const PullRequestOpenedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 87);

/**
 * Describes the message ket.remote.v1.MergePullRequest.
 * Use `create(MergePullRequestSchema)` to create a new message.
 */
export const MergePullRequestSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 88);

/**
 * Describes the message ket.remote.v1.PullRequestMerged.
 * Use `create(PullRequestMergedSchema)` to create a new message.
 */
export const PullRequestMergedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 89);

/**
 * Describes the message ket.remote.v1.UpdatePullRequest.
 * Use `create(UpdatePullRequestSchema)` to create a new message.
 */
export const UpdatePullRequestSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 90);

/**
 * Describes the message ket.remote.v1.PullRequestUpdated.
 * Use `create(PullRequestUpdatedSchema)` to create a new message.
 */
export const PullRequestUpdatedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 91);

/**
 * Describes the message ket.remote.v1.ReadyPullRequest.
 * Use `create(ReadyPullRequestSchema)` to create a new message.
 */
export const ReadyPullRequestSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 92);

/**
 * Describes the message ket.remote.v1.PullRequestReadied.
 * Use `create(PullRequestReadiedSchema)` to create a new message.
 */
export const PullRequestReadiedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 93);

/**
 * Describes the message ket.remote.v1.ReopenPullRequest.
 * Use `create(ReopenPullRequestSchema)` to create a new message.
 */
export const ReopenPullRequestSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 94);

/**
 * Describes the message ket.remote.v1.PullRequestReopened.
 * Use `create(PullRequestReopenedSchema)` to create a new message.
 */
export const PullRequestReopenedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 95);

/**
 * Describes the message ket.remote.v1.AddressComments.
 * Use `create(AddressCommentsSchema)` to create a new message.
 */
export const AddressCommentsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 96);

/**
 * Describes the message ket.remote.v1.CommentsHandedOver.
 * Use `create(CommentsHandedOverSchema)` to create a new message.
 */
export const CommentsHandedOverSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 97);

/**
 * Describes the message ket.remote.v1.OpenAgentSession.
 * Use `create(OpenAgentSessionSchema)` to create a new message.
 */
export const OpenAgentSessionSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 98);

/**
 * Describes the message ket.remote.v1.PinWorktree.
 * Use `create(PinWorktreeSchema)` to create a new message.
 */
export const PinWorktreeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 99);

/**
 * Describes the message ket.remote.v1.RenameWorktree.
 * Use `create(RenameWorktreeSchema)` to create a new message.
 */
export const RenameWorktreeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 100);

/**
 * Describes the message ket.remote.v1.DeleteWorktree.
 * Use `create(DeleteWorktreeSchema)` to create a new message.
 */
export const DeleteWorktreeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 101);

/**
 * Describes the message ket.remote.v1.WorktreeDeleted.
 * Use `create(WorktreeDeletedSchema)` to create a new message.
 */
export const WorktreeDeletedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 102);

/**
 * Describes the message ket.remote.v1.GetSnippets.
 * Use `create(GetSnippetsSchema)` to create a new message.
 */
export const GetSnippetsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 103);

/**
 * Describes the message ket.remote.v1.SnippetList.
 * Use `create(SnippetListSchema)` to create a new message.
 */
export const SnippetListSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 104);

/**
 * Describes the message ket.remote.v1.Snippet.
 * Use `create(SnippetSchema)` to create a new message.
 */
export const SnippetSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 105);

/**
 * Describes the message ket.remote.v1.ListDevices.
 * Use `create(ListDevicesSchema)` to create a new message.
 */
export const ListDevicesSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 106);

/**
 * Describes the message ket.remote.v1.DeviceList.
 * Use `create(DeviceListSchema)` to create a new message.
 */
export const DeviceListSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 107);

/**
 * Describes the message ket.remote.v1.Device.
 * Use `create(DeviceSchema)` to create a new message.
 */
export const DeviceSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 108);

/**
 * Describes the message ket.remote.v1.RevokeDevice.
 * Use `create(RevokeDeviceSchema)` to create a new message.
 */
export const RevokeDeviceSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 109);

/**
 * Describes the message ket.remote.v1.GetAutomations.
 * Use `create(GetAutomationsSchema)` to create a new message.
 */
export const GetAutomationsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 110);

/**
 * Describes the message ket.remote.v1.Automations.
 * Use `create(AutomationsSchema)` to create a new message.
 */
export const AutomationsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 111);

/**
 * Describes the message ket.remote.v1.AutomationSummary.
 * Use `create(AutomationSummarySchema)` to create a new message.
 */
export const AutomationSummarySchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 112);

/**
 * Describes the message ket.remote.v1.EditAutomation.
 * Use `create(EditAutomationSchema)` to create a new message.
 */
export const EditAutomationSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 113);

/**
 * Describes the enum ket.remote.v1.ErrorCode.
 */
export const ErrorCodeSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 0);

/**
 * @generated from enum ket.remote.v1.ErrorCode
 */
export const ErrorCode = /*@__PURE__*/
  tsEnum(ErrorCodeSchema);

/**
 * Describes the enum ket.remote.v1.UsageStatus.
 */
export const UsageStatusSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 1);

/**
 * Whether the numbers can be believed. A failed read is never a percentage:
 * "unavailable" and "0% used" must not look alike.
 *
 * @generated from enum ket.remote.v1.UsageStatus
 */
export const UsageStatus = /*@__PURE__*/
  tsEnum(UsageStatusSchema);

/**
 * Describes the enum ket.remote.v1.Activity.
 */
export const ActivitySchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 2);

/**
 * What a worktree's row is lit with: the same few states ket's sidebar
 * draws, so the phone and the desktop never disagree about what needs
 * somebody.
 *
 * @generated from enum ket.remote.v1.Activity
 */
export const Activity = /*@__PURE__*/
  tsEnum(ActivitySchema);

/**
 * Describes the enum ket.remote.v1.PushEnvironment.
 */
export const PushEnvironmentSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 3);

/**
 * Which of Apple's push services a token belongs to: a development build's
 * tokens work only with the sandbox.
 *
 * @generated from enum ket.remote.v1.PushEnvironment
 */
export const PushEnvironment = /*@__PURE__*/
  tsEnum(PushEnvironmentSchema);

/**
 * Describes the enum ket.remote.v1.PushKind.
 */
export const PushKindSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 4);

/**
 * What a phone asks to be told about, as its Settings › Notifications
 * lists them.
 *
 * @generated from enum ket.remote.v1.PushKind
 */
export const PushKind = /*@__PURE__*/
  tsEnum(PushKindSchema);

/**
 * Describes the enum ket.remote.v1.SwitchDepth.
 */
export const SwitchDepthSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 5);

/**
 * Since 1.39. How much of a terminal's conversation a switch hands over.
 *
 * @generated from enum ket.remote.v1.SwitchDepth
 */
export const SwitchDepth = /*@__PURE__*/
  tsEnum(SwitchDepthSchema);

/**
 * Describes the enum ket.remote.v1.BacklogPriority.
 */
export const BacklogPrioritySchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 6);

/**
 * Since 1.10. How pressing a note is. The list puts the most pressing first.
 *
 * @generated from enum ket.remote.v1.BacklogPriority
 */
export const BacklogPriority = /*@__PURE__*/
  tsEnum(BacklogPrioritySchema);

/**
 * Describes the enum ket.remote.v1.PullRequestMergeMethod.
 */
export const PullRequestMergeMethodSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 7);

/**
 * Since 1.19. How GitHub lands a pull request.
 *
 * @generated from enum ket.remote.v1.PullRequestMergeMethod
 */
export const PullRequestMergeMethod = /*@__PURE__*/
  tsEnum(PullRequestMergeMethodSchema);

/**
 * Describes the enum ket.remote.v1.AutomationEdit.
 */
export const AutomationEditSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 8);

/**
 * @generated from enum ket.remote.v1.AutomationEdit
 */
export const AutomationEdit = /*@__PURE__*/
  tsEnum(AutomationEditSchema);

