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

// @generated by protoc-gen-es v2.15.0 with parameter "target=js+dts,import_extension=js"
// @generated from file ket/remote/v1/remote.proto (package ket.remote.v1, syntax proto3)
/* eslint-disable */

import { enumDesc, fileDesc, messageDesc, tsEnum } from "@bufbuild/protobuf/codegenv2";

/**
 * Describes the file ket/remote/v1/remote.proto.
 */
export const file_ket_remote_v1_remote = /*@__PURE__*/
  fileDesc("ChprZXQvcmVtb3RlL3YxL3JlbW90ZS5wcm90bxINa2V0LnJlbW90ZS52MSKwHgoIRW52ZWxvcGUSDQoFbWFqb3IYASABKA0SDQoFbWlub3IYAiABKA0SEgoKcmVxdWVzdF9pZBgDIAEoBBIRCglzdHJlYW1faWQYBCABKAQSEgoKaG9zdF9lcG9jaBgFIAEoBBILCgNzZXEYBiABKAQSFwoPaWRlbXBvdGVuY3lfa2V5GAcgASgJEiUKBWhlbGxvGAogASgLMhQua2V0LnJlbW90ZS52MS5IZWxsb0gAEikKB3dlbGNvbWUYCyABKAsyFi5rZXQucmVtb3RlLnYxLldlbGNvbWVIABIlCgVlcnJvchgMIAEoCzIULmtldC5yZW1vdGUudjEuRXJyb3JIABIhCgNhY2sYDSABKAsyEi5rZXQucmVtb3RlLnYxLkFja0gAEjgKD3BhaXJpbmdfcGVuZGluZxgOIAEoCzIdLmtldC5yZW1vdGUudjEuUGFpcmluZ1BlbmRpbmdIABIvCghzbmFwc2hvdBgUIAEoCzIbLmtldC5yZW1vdGUudjEuSG9zdFNuYXBzaG90SAASLQoJc3Vic2NyaWJlGB4gASgLMhgua2V0LnJlbW90ZS52MS5TdWJzY3JpYmVIABIxCgt1bnN1YnNjcmliZRgfIAEoCzIaLmtldC5yZW1vdGUudjEuVW5zdWJzY3JpYmVIABI3CgpjaGVja3BvaW50GCAgASgLMiEua2V0LnJlbW90ZS52MS5UZXJtaW5hbENoZWNrcG9pbnRIABItCgVjaHVuaxghIAEoCzIcLmtldC5yZW1vdGUudjEuVGVybWluYWxDaHVua0gAEi0KBXJlc2V0GCIgASgLMhwua2V0LnJlbW90ZS52MS5UZXJtaW5hbFJlc2V0SAASLwoGY2xvc2VkGCMgASgLMh0ua2V0LnJlbW90ZS52MS5UZXJtaW5hbENsb3NlZEgAEiUKBWlucHV0GCggASgLMhQua2V0LnJlbW90ZS52MS5JbnB1dEgAEicKBnJlc2l6ZRgpIAEoCzIVLmtldC5yZW1vdGUudjEuUmVzaXplSAASJQoFbGVhc2UYKiABKAsyFC5rZXQucmVtb3RlLnYxLkxlYXNlSAASJwoGc2lnbmFsGCsgASgLMhUua2V0LnJlbW90ZS52MS5TaWduYWxIABIyCgxsaXN0X2RldmljZXMYMiABKAsyGi5rZXQucmVtb3RlLnYxLkxpc3REZXZpY2VzSAASLAoHZGV2aWNlcxgzIAEoCzIZLmtldC5yZW1vdGUudjEuRGV2aWNlTGlzdEgAEjQKDXJldm9rZV9kZXZpY2UYNCABKAsyGy5rZXQucmVtb3RlLnYxLlJldm9rZURldmljZUgAEicKBmFuc3dlchg8IAEoCzIVLmtldC5yZW1vdGUudjEuQW5zd2VySAASOgoQZ2V0X2NvbnZlcnNhdGlvbhhGIAEoCzIeLmtldC5yZW1vdGUudjEuR2V0Q29udmVyc2F0aW9uSAASMwoMY29udmVyc2F0aW9uGEcgASgLMhsua2V0LnJlbW90ZS52MS5Db252ZXJzYXRpb25IABIuCgpzdGFydF93b3JrGEggASgLMhgua2V0LnJlbW90ZS52MS5TdGFydFdvcmtIABIwCgtnZXRfY2hhbmdlcxhJIAEoCzIZLmtldC5yZW1vdGUudjEuR2V0Q2hhbmdlc0gAEikKB2NoYW5nZXMYSiABKAsyFi5rZXQucmVtb3RlLnYxLkNoYW5nZXNIABIqCghnZXRfZGlmZhhLIAEoCzIWLmtldC5yZW1vdGUudjEuR2V0RGlmZkgAEiMKBGRpZmYYTCABKAsyEy5rZXQucmVtb3RlLnYxLkRpZmZIABI2Cg5tZXJnZV93b3JrdHJlZRhNIAEoCzIcLmtldC5yZW1vdGUudjEuTWVyZ2VXb3JrdHJlZUgAEjIKDGdldF9zbmlwcGV0cxhOIAEoCzIaLmtldC5yZW1vdGUudjEuR2V0U25pcHBldHNIABIuCghzbmlwcGV0cxhPIAEoCzIaLmtldC5yZW1vdGUudjEuU25pcHBldExpc3RIABIpCgdyZXNwb25kGFAgASgLMhYua2V0LnJlbW90ZS52MS5SZXNwb25kSAASMAoLZ2V0X2JhY2tsb2cYWiABKAsyGS5rZXQucmVtb3RlLnYxLkdldEJhY2tsb2dIABIpCgdiYWNrbG9nGFsgASgLMhYua2V0LnJlbW90ZS52MS5CYWNrbG9nSAASMgoMZWRpdF9iYWNrbG9nGFwgASgLMhoua2V0LnJlbW90ZS52MS5FZGl0QmFja2xvZ0gAEi4KCnN0YXJ0X25vdGUYXSABKAsyGC5rZXQucmVtb3RlLnYxLlN0YXJ0Tm90ZUgAEi4KCmZpeF9jaGVja3MYZCABKAsyGC5rZXQucmVtb3RlLnYxLkZpeENoZWNrc0gAEj0KEmNoZWNrc19oYW5kZWRfb3ZlchhlIAEoCzIfLmtldC5yZW1vdGUudjEuQ2hlY2tzSGFuZGVkT3ZlckgAEi4KCnNlbmRfZmlsZXMYZiABKAsyGC5rZXQucmVtb3RlLnYxLlNlbmRGaWxlc0gAEjAKC3NlbmRfcmV2aWV3GGcgASgLMhkua2V0LnJlbW90ZS52MS5TZW5kUmV2aWV3SAASOwoRZ2V0X3N0YXJ0X29wdGlvbnMYaCABKAsyHi5rZXQucmVtb3RlLnYxLkdldFN0YXJ0T3B0aW9uc0gAEjQKDXN0YXJ0X29wdGlvbnMYaSABKAsyGy5rZXQucmVtb3RlLnYxLlN0YXJ0T3B0aW9uc0gAEjQKDXN0YXJ0X3Nlc3Npb24YaiABKAsyGy5rZXQucmVtb3RlLnYxLlN0YXJ0U2Vzc2lvbkgAEkQKFmdldF9wdWxsX3JlcXVlc3RfZHJhZnQYayABKAsyIi5rZXQucmVtb3RlLnYxLkdldFB1bGxSZXF1ZXN0RHJhZnRIABI9ChJwdWxsX3JlcXVlc3RfZHJhZnQYbCABKAsyHy5rZXQucmVtb3RlLnYxLlB1bGxSZXF1ZXN0RHJhZnRIABI7ChFvcGVuX3B1bGxfcmVxdWVzdBhtIAEoCzIeLmtldC5yZW1vdGUudjEuT3BlblB1bGxSZXF1ZXN0SAASPwoTcHVsbF9yZXF1ZXN0X29wZW5lZBhuIAEoCzIgLmtldC5yZW1vdGUudjEuUHVsbFJlcXVlc3RPcGVuZWRIABI9ChJtZXJnZV9wdWxsX3JlcXVlc3QYbyABKAsyHy5rZXQucmVtb3RlLnYxLk1lcmdlUHVsbFJlcXVlc3RIABI/ChNwdWxsX3JlcXVlc3RfbWVyZ2VkGHAgASgLMiAua2V0LnJlbW90ZS52MS5QdWxsUmVxdWVzdE1lcmdlZEgAEj8KE3VwZGF0ZV9wdWxsX3JlcXVlc3QYcSABKAsyIC5rZXQucmVtb3RlLnYxLlVwZGF0ZVB1bGxSZXF1ZXN0SAASQQoUcHVsbF9yZXF1ZXN0X3VwZGF0ZWQYciABKAsyIS5rZXQucmVtb3RlLnYxLlB1bGxSZXF1ZXN0VXBkYXRlZEgAEj0KEnJlYWR5X3B1bGxfcmVxdWVzdBhzIAEoCzIfLmtldC5yZW1vdGUudjEuUmVhZHlQdWxsUmVxdWVzdEgAEkEKFHB1bGxfcmVxdWVzdF9yZWFkaWVkGHQgASgLMiEua2V0LnJlbW90ZS52MS5QdWxsUmVxdWVzdFJlYWRpZWRIABI/ChNyZW9wZW5fcHVsbF9yZXF1ZXN0GHUgASgLMiAua2V0LnJlbW90ZS52MS5SZW9wZW5QdWxsUmVxdWVzdEgAEkMKFXB1bGxfcmVxdWVzdF9yZW9wZW5lZBh2IAEoCzIiLmtldC5yZW1vdGUudjEuUHVsbFJlcXVlc3RSZW9wZW5lZEgAEjoKEGFkZHJlc3NfY29tbWVudHMYdyABKAsyHi5rZXQucmVtb3RlLnYxLkFkZHJlc3NDb21tZW50c0gAEkEKFGNvbW1lbnRzX2hhbmRlZF9vdmVyGHggASgLMiEua2V0LnJlbW90ZS52MS5Db21tZW50c0hhbmRlZE92ZXJIABIyCgxwaW5fd29ya3RyZWUYeSABKAsyGi5rZXQucmVtb3RlLnYxLlBpbldvcmt0cmVlSAASOAoPZGVsZXRlX3dvcmt0cmVlGHogASgLMh0ua2V0LnJlbW90ZS52MS5EZWxldGVXb3JrdHJlZUgAEjoKEHdvcmt0cmVlX2RlbGV0ZWQYeyABKAsyHi5rZXQucmVtb3RlLnYxLldvcmt0cmVlRGVsZXRlZEgAEj0KEm9wZW5fYWdlbnRfc2Vzc2lvbhh8IAEoCzIfLmtldC5yZW1vdGUudjEuT3BlbkFnZW50U2Vzc2lvbkgAEjQKDXdha2VfdGVybWluYWwYfSABKAsyGy5rZXQucmVtb3RlLnYxLldha2VUZXJtaW5hbEgAEjYKDnRlcm1pbmFsX3dva2VuGH4gASgLMhwua2V0LnJlbW90ZS52MS5UZXJtaW5hbFdva2VuSAASKgoIc2V0X21vZGUYfyABKAsyFi5rZXQucmVtb3RlLnYxLlNldE1vZGVIABItCglzZXRfbW9kZWwYgAEgASgLMhcua2V0LnJlbW90ZS52MS5TZXRNb2RlbEgAEjEKC3N0YXJ0X2ZyZXNoGIEBIAEoCzIZLmtldC5yZW1vdGUudjEuU3RhcnRGcmVzaEgAEjUKDWZyZXNoX3N0YXJ0ZWQYggEgASgLMhsua2V0LnJlbW90ZS52MS5GcmVzaFN0YXJ0ZWRIABItCgltYXJrX3NlZW4YgwEgASgLMhcua2V0LnJlbW90ZS52MS5NYXJrU2VlbkgAEjkKD3JlbmFtZV93b3JrdHJlZRiEASABKAsyHS5rZXQucmVtb3RlLnYxLlJlbmFtZVdvcmt0cmVlSAASNQoNcmVnaXN0ZXJfcHVzaBiFASABKAsyGy5rZXQucmVtb3RlLnYxLlJlZ2lzdGVyUHVzaEgAEjkKD3VucmVnaXN0ZXJfcHVzaBiGASABKAsyHS5rZXQucmVtb3RlLnYxLlVucmVnaXN0ZXJQdXNoSAASMQoLbWFya191bnJlYWQYhwEgASgLMhkua2V0LnJlbW90ZS52MS5NYXJrVW5yZWFkSABCCQoHcGF5bG9hZCJbCgVIZWxsbxITCgtkZXZpY2VfbmFtZRgBIAEoCRITCgthcHBfdmVyc2lvbhgCIAEoCRIUCgxyZXN1bWVfZXBvY2gYAyABKAQSEgoKcmVzdW1lX3NlcRgEIAEoBCI3Cg5QYWlyaW5nUGVuZGluZxIMCgRjb2RlGAEgASgJEhcKD3RpbWVvdXRfc2Vjb25kcxgCIAEoDSJBCgdXZWxjb21lEhEKCWhvc3RfbmFtZRgBIAEoCRISCgpob3N0X2Vwb2NoGAIgASgEEg8KB3Jlc3VtZWQYAyABKAgiQAoFRXJyb3ISJgoEY29kZRgBIAEoDjIYLmtldC5yZW1vdGUudjEuRXJyb3JDb2RlEg8KB21lc3NhZ2UYAiABKAkiBQoDQWNrIrgBCgxIb3N0U25hcHNob3QSLwoIcHJvamVjdHMYASADKAsyHS5rZXQucmVtb3RlLnYxLlByb2plY3RTdW1tYXJ5EisKBXVzYWdlGAIgAygLMhwua2V0LnJlbW90ZS52MS5Qcm92aWRlclVzYWdlEg4KBmFnZW50cxgDIAMoCRInCgdwYWxldHRlGAQgASgLMhYua2V0LnJlbW90ZS52MS5QYWxldHRlEhEKCWFkZHJlc3NlcxgFIAMoCSKhAwoHUGFsZXR0ZRINCgVsaWdodBgBIAEoCBIQCghiYWNrZHJvcBgCIAEoCRIPCgdzdXJmYWNlGAMgASgJEg4KBnN1bmtlbhgEIAEoCRIQCghlbGV2YXRlZBgFIAEoCRINCgVob3ZlchgGIAEoCRIRCglzZWxlY3Rpb24YByABKAkSDgoGYm9yZGVyGAggASgJEgwKBHJ1bGUYCSABKAkSDgoGYWNjZW50GAogASgJEhEKCW9uX2FjY2VudBgLIAEoCRIMCgR0ZXh0GAwgASgJEgsKA2RpbRgNIAEoCRIOCgZtYXJrZXIYDiABKAkSDwoHcnVubmluZxgPIAEoCRIRCglhdHRlbnRpb24YECABKAkSDgoGZmFpbGVkGBEgASgJEg8KB21lcmdpbmcYEiABKAkSDQoFYWRkZWQYEyABKAkSDwoHcmVtb3ZlZBgUIAEoCRIQCghtb2RpZmllZBgVIAEoCRISCgpxdW90YV93YXJtGBYgASgJEhEKCXF1b3RhX2hvdBgXIAEoCRIWCg5xdW90YV9jcml0aWNhbBgYIAEoCSKvAQoNUHJvdmlkZXJVc2FnZRIQCghwcm92aWRlchgBIAEoCRIqCgZzdGF0dXMYAiABKA4yGi5rZXQucmVtb3RlLnYxLlVzYWdlU3RhdHVzEisKB3dpbmRvd3MYAyADKAsyGi5rZXQucmVtb3RlLnYxLlVzYWdlV2luZG93EgwKBHBsYW4YBCABKAkSFQoNZmV0Y2hlZF9hdF9tcxgFIAEoBBIOCgZyZWFzb24YBiABKAkiRwoLVXNhZ2VXaW5kb3cSDAoEbmFtZRgBIAEoCRIUCgx1c2VkX3BlcmNlbnQYAiABKAISFAoMcmVzZXRzX2F0X21zGAMgASgEIosBCg5Qcm9qZWN0U3VtbWFyeRIKCgJpZBgBIAEoCRIMCgRuYW1lGAIgASgJEjEKCXdvcmt0cmVlcxgDIAMoCzIeLmtldC5yZW1vdGUudjEuV29ya3RyZWVTdW1tYXJ5Eg0KBWNvbG9yGAQgASgJEg8KB2JhY2tsb2cYBSABKA0SDAoEaWNvbhgGIAEoCSLMBAoPV29ya3RyZWVTdW1tYXJ5EgoKAmlkGAEgASgJEgwKBG5hbWUYAiABKAkSDgoGYnJhbmNoGAMgASgJEhMKC2FnZW50X3N0YXRlGAQgASgJEjEKCXRlcm1pbmFscxgFIAMoCzIeLmtldC5yZW1vdGUudjEuVGVybWluYWxTdW1tYXJ5EikKCGFjdGl2aXR5GAYgASgOMhcua2V0LnJlbW90ZS52MS5BY3Rpdml0eRINCgVhZ2VudBgHIAEoCRIpCghxdWVzdGlvbhgIIAEoCzIXLmtldC5yZW1vdGUudjEuUXVlc3Rpb24SFQoNZmlsZXNfY2hhbmdlZBgJIAEoDRITCgtsaW5lc19hZGRlZBgKIAEoDRIVCg1saW5lc19yZW1vdmVkGAsgASgNEhUKDWNvbW1pdHNfYWhlYWQYDCABKA0SDAoEYmFzZRgNIAEoCRIxCglzdWJhZ2VudHMYDiADKAsyHi5rZXQucmVtb3RlLnYxLlN1YmFnZW50U3VtbWFyeRI3CgxwdWxsX3JlcXVlc3QYDyABKAsyIS5rZXQucmVtb3RlLnYxLlB1bGxSZXF1ZXN0U3VtbWFyeRIqCglxdWVzdGlvbnMYECADKAsyFy5rZXQucmVtb3RlLnYxLlF1ZXN0aW9uEg4KBnBpbm5lZBgRIAEoCBItCgZhc2xlZXAYEiADKAsyHS5rZXQucmVtb3RlLnYxLkFzbGVlcFRlcm1pbmFsEhMKC2FnZW50X2NvbG9yGBMgASgJEg4KBnVucmVhZBgUIAEoCCL7AwoSUHVsbFJlcXVlc3RTdW1tYXJ5Eg4KBm51bWJlchgBIAEoBBILCgN1cmwYAiABKAkSDQoFZHJhZnQYAyABKAgSOAoGcmV2aWV3GAQgASgOMigua2V0LnJlbW90ZS52MS5QdWxsUmVxdWVzdFN1bW1hcnkuUmV2aWV3EisKBmNoZWNrcxgFIAMoCzIbLmtldC5yZW1vdGUudjEuQ2hlY2tTdW1tYXJ5EjYKBXN0YXRlGAYgASgOMicua2V0LnJlbW90ZS52MS5QdWxsUmVxdWVzdFN1bW1hcnkuU3RhdGUSDAoEYmFzZRgHIAEoCRIOCgZiZWhpbmQYCCABKAgSGwoTdW5yZXNvbHZlZF9jb21tZW50cxgJIAEoDRIcChRtZXJnZV9jb21taXRfYWxsb3dlZBgKIAEoCBIcChRzcXVhc2hfbWVyZ2VfYWxsb3dlZBgLIAEoCBIcChRyZWJhc2VfbWVyZ2VfYWxsb3dlZBgMIAEoCCJaCgZSZXZpZXcSFgoSUkVWSUVXX1VOU1BFQ0lGSUVEEAASDAoIQVBQUk9WRUQQARIVChFDSEFOR0VTX1JFUVVFU1RFRBACEhMKD1JFVklFV19SRVFVSVJFRBADIikKBVN0YXRlEggKBE9QRU4QABIKCgZDTE9TRUQQARIKCgZNRVJHRUQQAiKtAQoMQ2hlY2tTdW1tYXJ5EgwKBG5hbWUYASABKAkSMAoFc3RhdGUYAiABKA4yIS5rZXQucmVtb3RlLnYxLkNoZWNrU3VtbWFyeS5TdGF0ZRILCgN1cmwYAyABKAkiUAoFU3RhdGUSFQoRU1RBVEVfVU5TUEVDSUZJRUQQABIKCgZQQVNTRUQQARIKCgZGQUlMRUQQAhILCgdQRU5ESU5HEAMSCwoHU0tJUFBFRBAEIssBCg9TdWJhZ2VudFN1bW1hcnkSCgoCaWQYASABKAkSDQoFbGFiZWwYAiABKAkSDQoFbW9kZWwYAyABKAkSMwoFc3RhdGUYBCABKA4yJC5rZXQucmVtb3RlLnYxLlN1YmFnZW50U3VtbWFyeS5TdGF0ZRIVCg1zdGFydGVkX2F0X21zGAUgASgEIkIKBVN0YXRlEhUKEVNUQVRFX1VOU1BFQ0lGSUVEEAASCwoHV09SS0lORxABEgsKB0JMT0NLRUQQAhIICgRJRExFEAMiwwEKCFF1ZXN0aW9uEgwKBHRvb2wYASABKAkSDwoHc3ViamVjdBgCIAEoCRISCgphbnN3ZXJhYmxlGAMgASgIEg4KBmFsd2F5cxgEIAEoCBIKCgJpZBgFIAEoBBITCgt0ZXJtaW5hbF9pZBgGIAEoBBIhCgRyaXNrGAcgASgLMhMua2V0LnJlbW90ZS52MS5SaXNrEiIKBGFza3MYCCADKAsyFC5rZXQucmVtb3RlLnYxLkFza2VkEgwKBHBsYW4YCSABKAgimwEKBUFza2VkEhAKCHF1ZXN0aW9uGAEgASgJEg4KBmhlYWRlchgCIAEoCRIsCgdjaG9pY2VzGAMgAygLMhsua2V0LnJlbW90ZS52MS5Bc2tlZC5DaG9pY2USFAoMbXVsdGlfc2VsZWN0GAQgASgIGiwKBkNob2ljZRINCgVsYWJlbBgBIAEoCRITCgtkZXNjcmlwdGlvbhgCIAEoCSIpCgRSaXNrEg0KBWxldmVsGAEgASgJEhIKCmNvbmZpZGVuY2UYAiABKAIiYgoOQXNsZWVwVGVybWluYWwSCwoDa2V5GAEgASgJEg0KBXRpdGxlGAIgASgJEg0KBWFnZW50GAMgASgJEhAKCHNpbmNlX21zGAQgASgEEhMKC2ZyZWVkX2J5dGVzGAUgASgEIjAKDFdha2VUZXJtaW5hbBITCgt3b3JrdHJlZV9pZBgBIAEoCRILCgNrZXkYAiABKAkiJAoNVGVybWluYWxXb2tlbhITCgt0ZXJtaW5hbF9pZBgBIAEoBCKEAwoPVGVybWluYWxTdW1tYXJ5EgoKAmlkGAEgASgEEg0KBXRpdGxlGAIgASgJEg0KBWFnZW50GAMgASgJEg4KBmNsb3NlZBgEIAEoCBIsCgphZ2VudF92aWV3GAUgASgLMhgua2V0LnJlbW90ZS52MS5BZ2VudFZpZXcSKAoFbW9kZXMYBiABKAsyGS5rZXQucmVtb3RlLnYxLkFnZW50TW9kZXMSLgoIc2V0dGluZ3MYByABKAsyHC5rZXQucmVtb3RlLnYxLkFnZW50U2V0dGluZ3MSJgoEY29sZBgIIAEoCzIYLmtldC5yZW1vdGUudjEuQ29sZENhY2hlEikKCGFjdGl2aXR5GAkgASgOMhcua2V0LnJlbW90ZS52MS5BY3Rpdml0eRIPCgdydW5uaW5nGAogASgJEhgKEHJ1bm5pbmdfc2luY2VfbXMYCyABKAQSEgoKY29tcGFjdGluZxgMIAEoCBINCgVjb2xvchgNIAEoCRIOCgZ1bnJlYWQYDiABKAgiHwoITWFya1NlZW4SEwoLdGVybWluYWxfaWQYASABKAQiMQoKTWFya1VucmVhZBITCgt3b3JrdHJlZV9pZBgBIAEoCRIOCgZ1bnJlYWQYAiABKAgijAEKDFJlZ2lzdGVyUHVzaBINCgV0b2tlbhgBIAEoDBIzCgtlbnZpcm9ubWVudBgCIAEoDjIeLmtldC5yZW1vdGUudjEuUHVzaEVudmlyb25tZW50EhAKCHB1c2hfa2V5GAMgASgMEiYKBWtpbmRzGAQgAygOMhcua2V0LnJlbW90ZS52MS5QdXNoS2luZCIQCg5VbnJlZ2lzdGVyUHVzaCJkCglDb2xkQ2FjaGUSDwoHc2Vzc2lvbhgBIAEoCRIQCghzaW5jZV9tcxgCIAEoBBIOCgZ0b2tlbnMYAyABKAQSEwoLY29zdF9taWNyb3MYBCABKAMSDwoHc3VtbWFyeRgFIAEoCSJECgpTdGFydEZyZXNoEhMKC3Rlcm1pbmFsX2lkGAEgASgEEg8KB3Nlc3Npb24YAiABKAkSEAoIc2luY2VfbXMYAyABKAQiIwoMRnJlc2hTdGFydGVkEhMKC3Rlcm1pbmFsX2lkGAEgASgEIkgKCkFnZW50TW9kZXMSDwoHY3VycmVudBgBIAEoCRIpCgdjaG9pY2VzGAIgAygLMhgua2V0LnJlbW90ZS52MS5BZ2VudE1vZGUiNgoJQWdlbnRNb2RlEgoKAmlkGAEgASgJEg0KBWxhYmVsGAIgASgJEg4KBmRldGFpbBgDIAEoCSIsCgdTZXRNb2RlEhMKC3Rlcm1pbmFsX2lkGAEgASgEEgwKBG1vZGUYAiABKAkingEKDUFnZW50U2V0dGluZ3MSDQoFbW9kZWwYASABKAkSDgoGZWZmb3J0GAIgASgJEioKBm1vZGVscxgDIAMoCzIaLmtldC5yZW1vdGUudjEuQWdlbnRDaG9pY2USKwoHZWZmb3J0cxgEIAMoCzIaLmtldC5yZW1vdGUudjEuQWdlbnRDaG9pY2USFQoNc2F2ZXNfZGVmYXVsdBgFIAEoCCIoCgtBZ2VudENob2ljZRIKCgJpZBgBIAEoCRINCgVsYWJlbBgCIAEoCSI+CghTZXRNb2RlbBITCgt0ZXJtaW5hbF9pZBgBIAEoBBINCgVtb2RlbBgCIAEoCRIOCgZlZmZvcnQYAyABKAkiTwoJQWdlbnRWaWV3EjIKCHNlc3Npb25zGAEgAygLMiAua2V0LnJlbW90ZS52MS5CYWNrZ3JvdW5kU2Vzc2lvbhIOCgZsaXN0ZWQYAiABKAgi6wEKEUJhY2tncm91bmRTZXNzaW9uEg8KB3Nlc3Npb24YASABKAkSDAoEbmFtZRgCIAEoCRI1CgVzdGF0ZRgDIAEoDjImLmtldC5yZW1vdGUudjEuQmFja2dyb3VuZFNlc3Npb24uU3RhdGUSFQoNc3RhcnRlZF9hdF9tcxgEIAEoAxIUCgxhY3RpdmVfYXRfbXMYBSABKAMSDQoFZG9pbmcYBiABKAkiRAoFU3RhdGUSFQoRU1RBVEVfVU5TUEVDSUZJRUQQABIICgRET05FEAESCwoHV09SS0lORxACEg0KCU5FRURTX1lPVRADIiAKCVN1YnNjcmliZRITCgt0ZXJtaW5hbF9pZBgBIAEoBCIiCgtVbnN1YnNjcmliZRITCgt0ZXJtaW5hbF9pZBgBIAEoBCKLAQoSVGVybWluYWxDaGVja3BvaW50EhMKC3Rlcm1pbmFsX2lkGAEgASgEEg0KBWVwb2NoGAIgASgEEhMKC3Rocm91Z2hfc2VxGAMgASgEEgwKBGNvbHMYBCABKA0SDAoEcm93cxgFIAEoDRISCgpzY3JvbGxiYWNrGAYgASgNEgwKBGFuc2kYByABKAwiTwoNVGVybWluYWxDaHVuaxITCgt0ZXJtaW5hbF9pZBgBIAEoBBINCgVlcG9jaBgCIAEoBBILCgNzZXEYAyABKAQSDQoFYnl0ZXMYBCABKAwiJAoNVGVybWluYWxSZXNldBITCgt0ZXJtaW5hbF9pZBgBIAEoBCJbCg5UZXJtaW5hbENsb3NlZBITCgt0ZXJtaW5hbF9pZBgBIAEoBBIWCglleGl0X2NvZGUYAiABKA1IAIgBARIOCgZzaWduYWwYAyABKAlCDAoKX2V4aXRfY29kZSIrCgVJbnB1dBITCgt0ZXJtaW5hbF9pZBgBIAEoBBINCgVieXRlcxgCIAEoDCJeCglTZW5kRmlsZXMSEwoLdGVybWluYWxfaWQYASABKAQSDAoEdGV4dBgCIAEoCRIuCgthdHRhY2htZW50cxgDIAMoCzIZLmtldC5yZW1vdGUudjEuQXR0YWNobWVudCIoCgpBdHRhY2htZW50EgwKBG5hbWUYASABKAkSDAoEZGF0YRgCIAEoDCI5CgZSZXNpemUSEwoLdGVybWluYWxfaWQYASABKAQSDAoEY29scxgCIAEoDRIMCgRyb3dzGAMgASgNIpABCgVMZWFzZRITCgt0ZXJtaW5hbF9pZBgBIAEoBBIrCgZhY3Rpb24YAiABKA4yGy5rZXQucmVtb3RlLnYxLkxlYXNlLkFjdGlvbiJFCgZBY3Rpb24SFgoSQUNUSU9OX1VOU1BFQ0lGSUVEEAASCwoHQUNRVUlSRRABEgkKBVJFTkVXEAISCwoHUkVMRUFTRRADIoMBCgZTaWduYWwSEwoLdGVybWluYWxfaWQYASABKAQSKAoEa2luZBgCIAEoDjIaLmtldC5yZW1vdGUudjEuU2lnbmFsLktpbmQiOgoES2luZBIUChBLSU5EX1VOU1BFQ0lGSUVEEAASDQoJSU5URVJSVVBUEAESDQoJVEVSTUlOQVRFEAIirgEKBkFuc3dlchITCgt0ZXJtaW5hbF9pZBgBIAEoBBIsCgZjaG9pY2UYAiABKA4yHC5rZXQucmVtb3RlLnYxLkFuc3dlci5DaG9pY2USEwoLcXVlc3Rpb25faWQYAyABKAQiTAoGQ2hvaWNlEhYKEkNIT0lDRV9VTlNQRUNJRklFRBAAEg4KCkFMTE9XX09OQ0UQARIQCgxBTExPV19BTFdBWVMQAhIICgRERU5ZEAMiqgEKB1Jlc3BvbmQSEwoLdGVybWluYWxfaWQYASABKAQSEwoLcXVlc3Rpb25faWQYAiABKAQSKgoFcGlja3MYAyADKAsyGy5rZXQucmVtb3RlLnYxLlJlc3BvbmQuUGljaxIPCgdhcHByb3ZlGAQgASgIEhAKCGZlZWRiYWNrGAUgASgJGiYKBFBpY2sSDwoHY2hvaWNlcxgBIAMoDRINCgVvdGhlchgCIAEoCSJGCg9HZXRDb252ZXJzYXRpb24SEwoLdGVybWluYWxfaWQYASABKAQSDQoFYWZ0ZXIYAiABKAQSDwoHc2Vzc2lvbhgDIAEoCSKoAQoMQ29udmVyc2F0aW9uEhMKC3Rlcm1pbmFsX2lkGAEgASgEEhEKCXN1cHBvcnRlZBgCIAEoCBIOCgZyZWFzb24YAyABKAkSDwoHc2Vzc2lvbhgEIAEoCRIiCgV0dXJucxgFIAMoCzITLmtldC5yZW1vdGUudjEuVHVybhIMCgRuZXh0GAYgASgEEg0KBWZyZXNoGAcgASgIEg4KBnF1ZXVlZBgIIAMoCSKbAQoEVHVybhImCgRraW5kGAEgASgOMhgua2V0LnJlbW90ZS52MS5UdXJuLktpbmQSDAoEdGV4dBgCIAEoCRIMCgR0b29sGAMgASgJEg4KBmltYWdlcxgEIAEoDSI/CgRLaW5kEhQKEEtJTkRfVU5TUEVDSUZJRUQQABIICgRVU0VSEAESDQoJQVNTSVNUQU5UEAISCAoEVE9PTBADImsKCVN0YXJ0V29yaxISCgpwcm9qZWN0X2lkGAEgASgJEg4KBnByb21wdBgCIAEoCRINCgVhZ2VudBgDIAEoCRIMCgRiYXNlGAQgASgJEg0KBW1vZGVsGAUgASgJEg4KBmVmZm9ydBgGIAEoCSIlCg9HZXRTdGFydE9wdGlvbnMSEgoKcHJvamVjdF9pZBgBIAEoCSJDChFBZ2VudFN0YXJ0T3B0aW9ucxINCgVhZ2VudBgBIAEoCRIOCgZtb2RlbHMYAiADKAkSDwoHZWZmb3J0cxgDIAMoCSKCAQoMU3RhcnRPcHRpb25zEhIKCnByb2plY3RfaWQYASABKAkSFAoMZGVmYXVsdF9iYXNlGAIgASgJEhYKDmxvY2FsX2JyYW5jaGVzGAMgAygJEjAKBmFnZW50cxgEIAMoCzIgLmtldC5yZW1vdGUudjEuQWdlbnRTdGFydE9wdGlvbnMiYQoMU3RhcnRTZXNzaW9uEhMKC3dvcmt0cmVlX2lkGAEgASgJEg0KBWFnZW50GAIgASgJEg4KBnByb21wdBgDIAEoCRINCgVtb2RlbBgEIAEoCRIOCgZlZmZvcnQYBSABKAkiIAoKR2V0QmFja2xvZxISCgpwcm9qZWN0X2lkGAEgASgJIkgKB0JhY2tsb2cSEgoKcHJvamVjdF9pZBgBIAEoCRIpCgVub3RlcxgCIAMoCzIaLmtldC5yZW1vdGUudjEuQmFja2xvZ05vdGUiwAEKC0JhY2tsb2dOb3RlEgoKAmlkGAEgASgJEg0KBXRpdGxlGAIgASgJEgwKBGJvZHkYAyABKAkSDQoFZmlsZXMYBCADKAkSEgoKY3JlYXRlZF9tcxgFIAEoBBISCgp1cGRhdGVkX21zGAYgASgEEg8KB2RvbmVfbXMYByABKAQSDgoGYnJhbmNoGAggASgJEjAKCHByaW9yaXR5GAkgASgOMh4ua2V0LnJlbW90ZS52MS5CYWNrbG9nUHJpb3JpdHkiggIKC0VkaXRCYWNrbG9nEhIKCnByb2plY3RfaWQYASABKAkSDwoHbm90ZV9pZBgCIAEoCRIxCgZhY3Rpb24YAyABKA4yIS5rZXQucmVtb3RlLnYxLkVkaXRCYWNrbG9nLkFjdGlvbhINCgV0aXRsZRgEIAEoCRIMCgRib2R5GAUgASgJEjAKCHByaW9yaXR5GAYgASgOMh4ua2V0LnJlbW90ZS52MS5CYWNrbG9nUHJpb3JpdHkiTAoGQWN0aW9uEhYKEkFDVElPTl9VTlNQRUNJRklFRBAAEggKBFNBVkUQARIICgRET05FEAISCgoGUkVPUEVOEAMSCgoGUkVNT1ZFEAQiMAoJU3RhcnROb3RlEhIKCnByb2plY3RfaWQYASABKAkSDwoHbm90ZV9pZBgCIAEoCSIhCgpHZXRDaGFuZ2VzEhMKC3dvcmt0cmVlX2lkGAEgASgJImUKB0NoYW5nZXMSEwoLd29ya3RyZWVfaWQYASABKAkSKAoFZmlsZXMYAiADKAsyGS5rZXQucmVtb3RlLnYxLkZpbGVDaGFuZ2USDAoEYmFzZRgDIAEoCRINCgVlcnJvchgEIAEoCSLEAQoKRmlsZUNoYW5nZRIMCgRwYXRoGAEgASgJEg0KBWFkZGVkGAIgASgNEg8KB3JlbW92ZWQYAyABKA0SMAoGc3RhdHVzGAQgASgOMiAua2V0LnJlbW90ZS52MS5GaWxlQ2hhbmdlLlN0YXR1cxIOCgZiaW5hcnkYBSABKAgiRgoGU3RhdHVzEhYKElNUQVRVU19VTlNQRUNJRklFRBAAEgwKCE1PRElGSUVEEAESCQoFQURERUQQAhILCgdERUxFVEVEEAMiLAoHR2V0RGlmZhITCgt3b3JrdHJlZV9pZBgBIAEoCRIMCgRwYXRoGAIgASgJIlkKBERpZmYSEwoLd29ya3RyZWVfaWQYASABKAkSDAoEcGF0aBgCIAEoCRIMCgR0ZXh0GAMgASgJEhEKCXRydW5jYXRlZBgEIAEoCBINCgVlcnJvchgFIAEoCSIkCg1NZXJnZVdvcmt0cmVlEhMKC3dvcmt0cmVlX2lkGAEgASgJIiAKCUZpeENoZWNrcxITCgt3b3JrdHJlZV9pZBgBIAEoCSIkChBDaGVja3NIYW5kZWRPdmVyEhAKCHN0YXJ0aW5nGAEgASgIIlEKClNlbmRSZXZpZXcSEwoLd29ya3RyZWVfaWQYASABKAkSLgoIY29tbWVudHMYAiADKAsyHC5rZXQucmVtb3RlLnYxLlJldmlld0NvbW1lbnQiSwoNUmV2aWV3Q29tbWVudBIMCgRwYXRoGAEgASgJEg0KBWxpbmVzGAIgASgJEgwKBHRleHQYAyABKAkSDwoHY29tbWVudBgEIAEoCSIqChNHZXRQdWxsUmVxdWVzdERyYWZ0EhMKC3dvcmt0cmVlX2lkGAEgASgJIrIBChBQdWxsUmVxdWVzdERyYWZ0EhMKC3dvcmt0cmVlX2lkGAEgASgJEg4KBmJyYW5jaBgCIAEoCRIMCgRiYXNlGAMgASgJEg4KBnJlbW90ZRgEIAEoCRINCgV0aXRsZRgFIAEoCRIMCgRib2R5GAYgASgJEg8KB2NoYW5nZXMYByABKA0SFwoPZXhpc3RpbmdfbnVtYmVyGAggASgEEhQKDGV4aXN0aW5nX3VybBgJIAEoCSJSCg9PcGVuUHVsbFJlcXVlc3QSEwoLd29ya3RyZWVfaWQYASABKAkSDQoFdGl0bGUYAiABKAkSDAoEYm9keRgDIAEoCRINCgVkcmFmdBgEIAEoCCKCAQoRUHVsbFJlcXVlc3RPcGVuZWQSDgoGbnVtYmVyGAEgASgEEgsKA3VybBgCIAEoCRIOCgZicmFuY2gYAyABKAkSDAoEYmFzZRgEIAEoCRIOCgZyZW1vdGUYBSABKAkSDwoHZXhpc3RlZBgGIAEoCBIRCgljb21taXR0ZWQYByABKA0iXgoQTWVyZ2VQdWxsUmVxdWVzdBITCgt3b3JrdHJlZV9pZBgBIAEoCRI1CgZtZXRob2QYAiABKA4yJS5rZXQucmVtb3RlLnYxLlB1bGxSZXF1ZXN0TWVyZ2VNZXRob2QiZwoRUHVsbFJlcXVlc3RNZXJnZWQSDgoGbnVtYmVyGAEgASgEEgsKA3VybBgCIAEoCRI1CgZtZXRob2QYAyABKA4yJS5rZXQucmVtb3RlLnYxLlB1bGxSZXF1ZXN0TWVyZ2VNZXRob2QiKAoRVXBkYXRlUHVsbFJlcXVlc3QSEwoLd29ya3RyZWVfaWQYASABKAkiJAoSUHVsbFJlcXVlc3RVcGRhdGVkEg4KBm51bWJlchgBIAEoBCInChBSZWFkeVB1bGxSZXF1ZXN0EhMKC3dvcmt0cmVlX2lkGAEgASgJIiQKElB1bGxSZXF1ZXN0UmVhZGllZBIOCgZudW1iZXIYASABKAQiKAoRUmVvcGVuUHVsbFJlcXVlc3QSEwoLd29ya3RyZWVfaWQYASABKAkiJQoTUHVsbFJlcXVlc3RSZW9wZW5lZBIOCgZudW1iZXIYASABKAQiJgoPQWRkcmVzc0NvbW1lbnRzEhMKC3dvcmt0cmVlX2lkGAEgASgJIiYKEkNvbW1lbnRzSGFuZGVkT3ZlchIQCghzdGFydGluZxgBIAEoCCJGChBPcGVuQWdlbnRTZXNzaW9uEhMKC3Rlcm1pbmFsX2lkGAEgASgEEg8KB3Nlc3Npb24YAiABKAkSDAoEdGV4dBgDIAEoCSIyCgtQaW5Xb3JrdHJlZRITCgt3b3JrdHJlZV9pZBgBIAEoCRIOCgZwaW5uZWQYAiABKAgiMwoOUmVuYW1lV29ya3RyZWUSEwoLd29ya3RyZWVfaWQYASABKAkSDAoEbmFtZRgCIAEoCSI0Cg5EZWxldGVXb3JrdHJlZRITCgt3b3JrdHJlZV9pZBgBIAEoCRINCgVmb3JjZRgCIAEoCCImCg9Xb3JrdHJlZURlbGV0ZWQSEwoLa2VwdF9icmFuY2gYASABKAkiDQoLR2V0U25pcHBldHMiNwoLU25pcHBldExpc3QSKAoIc25pcHBldHMYASADKAsyFi5rZXQucmVtb3RlLnYxLlNuaXBwZXQiJQoHU25pcHBldBIMCgRuYW1lGAEgASgJEgwKBGJvZHkYAiABKAkiDQoLTGlzdERldmljZXMiNAoKRGV2aWNlTGlzdBImCgdkZXZpY2VzGAEgAygLMhUua2V0LnJlbW90ZS52MS5EZXZpY2UiawoGRGV2aWNlEgoKAmlkGAEgASgJEgwKBG5hbWUYAiABKAkSEQoJcGFpcmVkX2F0GAMgASgEEhMKC3RoaXNfZGV2aWNlGAQgASgIEhEKCWNvbm5lY3RlZBgFIAEoCBIMCgRwdXNoGAYgASgIIhoKDFJldm9rZURldmljZRIKCgJpZBgBIAEoCSq9AQoJRXJyb3JDb2RlEhoKFkVSUk9SX0NPREVfVU5TUEVDSUZJRUQQABIQCgxVTkFVVEhPUklaRUQQARIRCg1TVEFMRV9SRVFVRVNUEAISDQoJTk9UX0ZPVU5EEAMSEgoOTk9UX0NPTlRST0xMRVIQBBIQCgxCQUNLUFJFU1NVUkUQBRIXChNVTlNVUFBPUlRFRF9WRVJTSU9OEAYSDAoIREVDTElORUQQBxITCg9XT1VMRF9MT1NFX1dPUksQCCqXAQoLVXNhZ2VTdGF0dXMSHAoYVVNBR0VfU1RBVFVTX1VOU1BFQ0lGSUVEEAASFgoSVVNBR0VfU1RBVFVTX0ZSRVNIEAESFgoSVVNBR0VfU1RBVFVTX1NUQUxFEAISHAoYVVNBR0VfU1RBVFVTX1VOQVZBSUxBQkxFEAMSHAoYVVNBR0VfU1RBVFVTX1VOU1VQUE9SVEVEEAQqiwEKCEFjdGl2aXR5EhIKDkFDVElWSVRZX1FVSUVUEAASFAoQQUNUSVZJVFlfUlVOTklORxABEhQKEEFDVElWSVRZX1dPUktJTkcQAhIUChBBQ1RJVklUWV9NRVJHSU5HEAMSFAoQQUNUSVZJVFlfQkxPQ0tFRBAEEhMKD0FDVElWSVRZX0ZBSUxFRBAFKnIKD1B1c2hFbnZpcm9ubWVudBIgChxQVVNIX0VOVklST05NRU5UX1VOU1BFQ0lGSUVEEAASHAoYUFVTSF9FTlZJUk9OTUVOVF9TQU5EQk9YEAESHwobUFVTSF9FTlZJUk9OTUVOVF9QUk9EVUNUSU9OEAIqagoIUHVzaEtpbmQSGQoVUFVTSF9LSU5EX1VOU1BFQ0lGSUVEEAASFQoRUFVTSF9LSU5EX1dBSVRJTkcQARIWChJQVVNIX0tJTkRfRklOSVNIRUQQAhIUChBQVVNIX0tJTkRfQ0hFQ0tTEAMqogEKD0JhY2tsb2dQcmlvcml0eRIgChxCQUNLTE9HX1BSSU9SSVRZX1VOU1BFQ0lGSUVEEAASGAoUQkFDS0xPR19QUklPUklUWV9MT1cQARIbChdCQUNLTE9HX1BSSU9SSVRZX01FRElVTRACEhkKFUJBQ0tMT0dfUFJJT1JJVFlfSElHSBADEhsKF0JBQ0tMT0dfUFJJT1JJVFlfVVJHRU5UEAQqbQoWUHVsbFJlcXVlc3RNZXJnZU1ldGhvZBIpCiVQVUxMX1JFUVVFU1RfTUVSR0VfTUVUSE9EX1VOU1BFQ0lGSUVEEAASEAoMTUVSR0VfQ09NTUlUEAESCgoGU1FVQVNIEAISCgoGUkVCQVNFEANiBnByb3RvMw");

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
 * Describes the message ket.remote.v1.Palette.
 * Use `create(PaletteSchema)` to create a new message.
 */
export const PaletteSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 7);

/**
 * Describes the message ket.remote.v1.ProviderUsage.
 * Use `create(ProviderUsageSchema)` to create a new message.
 */
export const ProviderUsageSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 8);

/**
 * Describes the message ket.remote.v1.UsageWindow.
 * Use `create(UsageWindowSchema)` to create a new message.
 */
export const UsageWindowSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 9);

/**
 * Describes the message ket.remote.v1.ProjectSummary.
 * Use `create(ProjectSummarySchema)` to create a new message.
 */
export const ProjectSummarySchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 10);

/**
 * Describes the message ket.remote.v1.WorktreeSummary.
 * Use `create(WorktreeSummarySchema)` to create a new message.
 */
export const WorktreeSummarySchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 11);

/**
 * Describes the message ket.remote.v1.PullRequestSummary.
 * Use `create(PullRequestSummarySchema)` to create a new message.
 */
export const PullRequestSummarySchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 12);

/**
 * Describes the enum ket.remote.v1.PullRequestSummary.Review.
 */
export const PullRequestSummary_ReviewSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 12, 0);

/**
 * @generated from enum ket.remote.v1.PullRequestSummary.Review
 */
export const PullRequestSummary_Review = /*@__PURE__*/
  tsEnum(PullRequestSummary_ReviewSchema);

/**
 * Describes the enum ket.remote.v1.PullRequestSummary.State.
 */
export const PullRequestSummary_StateSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 12, 1);

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
  messageDesc(file_ket_remote_v1_remote, 13);

/**
 * Describes the enum ket.remote.v1.CheckSummary.State.
 */
export const CheckSummary_StateSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 13, 0);

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
  messageDesc(file_ket_remote_v1_remote, 14);

/**
 * Describes the enum ket.remote.v1.SubagentSummary.State.
 */
export const SubagentSummary_StateSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 14, 0);

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
  messageDesc(file_ket_remote_v1_remote, 15);

/**
 * Describes the message ket.remote.v1.Asked.
 * Use `create(AskedSchema)` to create a new message.
 */
export const AskedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 16);

/**
 * Describes the message ket.remote.v1.Asked.Choice.
 * Use `create(Asked_ChoiceSchema)` to create a new message.
 */
export const Asked_ChoiceSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 16, 0);

/**
 * Describes the message ket.remote.v1.Risk.
 * Use `create(RiskSchema)` to create a new message.
 */
export const RiskSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 17);

/**
 * Describes the message ket.remote.v1.AsleepTerminal.
 * Use `create(AsleepTerminalSchema)` to create a new message.
 */
export const AsleepTerminalSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 18);

/**
 * Describes the message ket.remote.v1.WakeTerminal.
 * Use `create(WakeTerminalSchema)` to create a new message.
 */
export const WakeTerminalSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 19);

/**
 * Describes the message ket.remote.v1.TerminalWoken.
 * Use `create(TerminalWokenSchema)` to create a new message.
 */
export const TerminalWokenSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 20);

/**
 * Describes the message ket.remote.v1.TerminalSummary.
 * Use `create(TerminalSummarySchema)` to create a new message.
 */
export const TerminalSummarySchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 21);

/**
 * Describes the message ket.remote.v1.MarkSeen.
 * Use `create(MarkSeenSchema)` to create a new message.
 */
export const MarkSeenSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 22);

/**
 * Describes the message ket.remote.v1.MarkUnread.
 * Use `create(MarkUnreadSchema)` to create a new message.
 */
export const MarkUnreadSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 23);

/**
 * Describes the message ket.remote.v1.RegisterPush.
 * Use `create(RegisterPushSchema)` to create a new message.
 */
export const RegisterPushSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 24);

/**
 * Describes the message ket.remote.v1.UnregisterPush.
 * Use `create(UnregisterPushSchema)` to create a new message.
 */
export const UnregisterPushSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 25);

/**
 * Describes the message ket.remote.v1.ColdCache.
 * Use `create(ColdCacheSchema)` to create a new message.
 */
export const ColdCacheSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 26);

/**
 * Describes the message ket.remote.v1.StartFresh.
 * Use `create(StartFreshSchema)` to create a new message.
 */
export const StartFreshSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 27);

/**
 * Describes the message ket.remote.v1.FreshStarted.
 * Use `create(FreshStartedSchema)` to create a new message.
 */
export const FreshStartedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 28);

/**
 * Describes the message ket.remote.v1.AgentModes.
 * Use `create(AgentModesSchema)` to create a new message.
 */
export const AgentModesSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 29);

/**
 * Describes the message ket.remote.v1.AgentMode.
 * Use `create(AgentModeSchema)` to create a new message.
 */
export const AgentModeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 30);

/**
 * Describes the message ket.remote.v1.SetMode.
 * Use `create(SetModeSchema)` to create a new message.
 */
export const SetModeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 31);

/**
 * Describes the message ket.remote.v1.AgentSettings.
 * Use `create(AgentSettingsSchema)` to create a new message.
 */
export const AgentSettingsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 32);

/**
 * Describes the message ket.remote.v1.AgentChoice.
 * Use `create(AgentChoiceSchema)` to create a new message.
 */
export const AgentChoiceSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 33);

/**
 * Describes the message ket.remote.v1.SetModel.
 * Use `create(SetModelSchema)` to create a new message.
 */
export const SetModelSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 34);

/**
 * Describes the message ket.remote.v1.AgentView.
 * Use `create(AgentViewSchema)` to create a new message.
 */
export const AgentViewSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 35);

/**
 * Describes the message ket.remote.v1.BackgroundSession.
 * Use `create(BackgroundSessionSchema)` to create a new message.
 */
export const BackgroundSessionSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 36);

/**
 * Describes the enum ket.remote.v1.BackgroundSession.State.
 */
export const BackgroundSession_StateSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 36, 0);

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
  messageDesc(file_ket_remote_v1_remote, 37);

/**
 * Describes the message ket.remote.v1.Unsubscribe.
 * Use `create(UnsubscribeSchema)` to create a new message.
 */
export const UnsubscribeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 38);

/**
 * Describes the message ket.remote.v1.TerminalCheckpoint.
 * Use `create(TerminalCheckpointSchema)` to create a new message.
 */
export const TerminalCheckpointSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 39);

/**
 * Describes the message ket.remote.v1.TerminalChunk.
 * Use `create(TerminalChunkSchema)` to create a new message.
 */
export const TerminalChunkSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 40);

/**
 * Describes the message ket.remote.v1.TerminalReset.
 * Use `create(TerminalResetSchema)` to create a new message.
 */
export const TerminalResetSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 41);

/**
 * Describes the message ket.remote.v1.TerminalClosed.
 * Use `create(TerminalClosedSchema)` to create a new message.
 */
export const TerminalClosedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 42);

/**
 * Describes the message ket.remote.v1.Input.
 * Use `create(InputSchema)` to create a new message.
 */
export const InputSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 43);

/**
 * Describes the message ket.remote.v1.SendFiles.
 * Use `create(SendFilesSchema)` to create a new message.
 */
export const SendFilesSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 44);

/**
 * Describes the message ket.remote.v1.Attachment.
 * Use `create(AttachmentSchema)` to create a new message.
 */
export const AttachmentSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 45);

/**
 * Describes the message ket.remote.v1.Resize.
 * Use `create(ResizeSchema)` to create a new message.
 */
export const ResizeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 46);

/**
 * Describes the message ket.remote.v1.Lease.
 * Use `create(LeaseSchema)` to create a new message.
 */
export const LeaseSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 47);

/**
 * Describes the enum ket.remote.v1.Lease.Action.
 */
export const Lease_ActionSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 47, 0);

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
  messageDesc(file_ket_remote_v1_remote, 48);

/**
 * Describes the enum ket.remote.v1.Signal.Kind.
 */
export const Signal_KindSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 48, 0);

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
  messageDesc(file_ket_remote_v1_remote, 49);

/**
 * Describes the enum ket.remote.v1.Answer.Choice.
 */
export const Answer_ChoiceSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 49, 0);

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
  messageDesc(file_ket_remote_v1_remote, 50);

/**
 * Describes the message ket.remote.v1.Respond.Pick.
 * Use `create(Respond_PickSchema)` to create a new message.
 */
export const Respond_PickSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 50, 0);

/**
 * Describes the message ket.remote.v1.GetConversation.
 * Use `create(GetConversationSchema)` to create a new message.
 */
export const GetConversationSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 51);

/**
 * Describes the message ket.remote.v1.Conversation.
 * Use `create(ConversationSchema)` to create a new message.
 */
export const ConversationSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 52);

/**
 * Describes the message ket.remote.v1.Turn.
 * Use `create(TurnSchema)` to create a new message.
 */
export const TurnSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 53);

/**
 * Describes the enum ket.remote.v1.Turn.Kind.
 */
export const Turn_KindSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 53, 0);

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
  messageDesc(file_ket_remote_v1_remote, 54);

/**
 * Describes the message ket.remote.v1.GetStartOptions.
 * Use `create(GetStartOptionsSchema)` to create a new message.
 */
export const GetStartOptionsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 55);

/**
 * Describes the message ket.remote.v1.AgentStartOptions.
 * Use `create(AgentStartOptionsSchema)` to create a new message.
 */
export const AgentStartOptionsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 56);

/**
 * Describes the message ket.remote.v1.StartOptions.
 * Use `create(StartOptionsSchema)` to create a new message.
 */
export const StartOptionsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 57);

/**
 * Describes the message ket.remote.v1.StartSession.
 * Use `create(StartSessionSchema)` to create a new message.
 */
export const StartSessionSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 58);

/**
 * Describes the message ket.remote.v1.GetBacklog.
 * Use `create(GetBacklogSchema)` to create a new message.
 */
export const GetBacklogSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 59);

/**
 * Describes the message ket.remote.v1.Backlog.
 * Use `create(BacklogSchema)` to create a new message.
 */
export const BacklogSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 60);

/**
 * Describes the message ket.remote.v1.BacklogNote.
 * Use `create(BacklogNoteSchema)` to create a new message.
 */
export const BacklogNoteSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 61);

/**
 * Describes the message ket.remote.v1.EditBacklog.
 * Use `create(EditBacklogSchema)` to create a new message.
 */
export const EditBacklogSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 62);

/**
 * Describes the enum ket.remote.v1.EditBacklog.Action.
 */
export const EditBacklog_ActionSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 62, 0);

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
  messageDesc(file_ket_remote_v1_remote, 63);

/**
 * Describes the message ket.remote.v1.GetChanges.
 * Use `create(GetChangesSchema)` to create a new message.
 */
export const GetChangesSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 64);

/**
 * Describes the message ket.remote.v1.Changes.
 * Use `create(ChangesSchema)` to create a new message.
 */
export const ChangesSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 65);

/**
 * Describes the message ket.remote.v1.FileChange.
 * Use `create(FileChangeSchema)` to create a new message.
 */
export const FileChangeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 66);

/**
 * Describes the enum ket.remote.v1.FileChange.Status.
 */
export const FileChange_StatusSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 66, 0);

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
  messageDesc(file_ket_remote_v1_remote, 67);

/**
 * Describes the message ket.remote.v1.Diff.
 * Use `create(DiffSchema)` to create a new message.
 */
export const DiffSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 68);

/**
 * Describes the message ket.remote.v1.MergeWorktree.
 * Use `create(MergeWorktreeSchema)` to create a new message.
 */
export const MergeWorktreeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 69);

/**
 * Describes the message ket.remote.v1.FixChecks.
 * Use `create(FixChecksSchema)` to create a new message.
 */
export const FixChecksSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 70);

/**
 * Describes the message ket.remote.v1.ChecksHandedOver.
 * Use `create(ChecksHandedOverSchema)` to create a new message.
 */
export const ChecksHandedOverSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 71);

/**
 * Describes the message ket.remote.v1.SendReview.
 * Use `create(SendReviewSchema)` to create a new message.
 */
export const SendReviewSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 72);

/**
 * Describes the message ket.remote.v1.ReviewComment.
 * Use `create(ReviewCommentSchema)` to create a new message.
 */
export const ReviewCommentSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 73);

/**
 * Describes the message ket.remote.v1.GetPullRequestDraft.
 * Use `create(GetPullRequestDraftSchema)` to create a new message.
 */
export const GetPullRequestDraftSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 74);

/**
 * Describes the message ket.remote.v1.PullRequestDraft.
 * Use `create(PullRequestDraftSchema)` to create a new message.
 */
export const PullRequestDraftSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 75);

/**
 * Describes the message ket.remote.v1.OpenPullRequest.
 * Use `create(OpenPullRequestSchema)` to create a new message.
 */
export const OpenPullRequestSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 76);

/**
 * Describes the message ket.remote.v1.PullRequestOpened.
 * Use `create(PullRequestOpenedSchema)` to create a new message.
 */
export const PullRequestOpenedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 77);

/**
 * Describes the message ket.remote.v1.MergePullRequest.
 * Use `create(MergePullRequestSchema)` to create a new message.
 */
export const MergePullRequestSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 78);

/**
 * Describes the message ket.remote.v1.PullRequestMerged.
 * Use `create(PullRequestMergedSchema)` to create a new message.
 */
export const PullRequestMergedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 79);

/**
 * Describes the message ket.remote.v1.UpdatePullRequest.
 * Use `create(UpdatePullRequestSchema)` to create a new message.
 */
export const UpdatePullRequestSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 80);

/**
 * Describes the message ket.remote.v1.PullRequestUpdated.
 * Use `create(PullRequestUpdatedSchema)` to create a new message.
 */
export const PullRequestUpdatedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 81);

/**
 * Describes the message ket.remote.v1.ReadyPullRequest.
 * Use `create(ReadyPullRequestSchema)` to create a new message.
 */
export const ReadyPullRequestSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 82);

/**
 * Describes the message ket.remote.v1.PullRequestReadied.
 * Use `create(PullRequestReadiedSchema)` to create a new message.
 */
export const PullRequestReadiedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 83);

/**
 * Describes the message ket.remote.v1.ReopenPullRequest.
 * Use `create(ReopenPullRequestSchema)` to create a new message.
 */
export const ReopenPullRequestSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 84);

/**
 * Describes the message ket.remote.v1.PullRequestReopened.
 * Use `create(PullRequestReopenedSchema)` to create a new message.
 */
export const PullRequestReopenedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 85);

/**
 * Describes the message ket.remote.v1.AddressComments.
 * Use `create(AddressCommentsSchema)` to create a new message.
 */
export const AddressCommentsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 86);

/**
 * Describes the message ket.remote.v1.CommentsHandedOver.
 * Use `create(CommentsHandedOverSchema)` to create a new message.
 */
export const CommentsHandedOverSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 87);

/**
 * Describes the message ket.remote.v1.OpenAgentSession.
 * Use `create(OpenAgentSessionSchema)` to create a new message.
 */
export const OpenAgentSessionSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 88);

/**
 * Describes the message ket.remote.v1.PinWorktree.
 * Use `create(PinWorktreeSchema)` to create a new message.
 */
export const PinWorktreeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 89);

/**
 * Describes the message ket.remote.v1.RenameWorktree.
 * Use `create(RenameWorktreeSchema)` to create a new message.
 */
export const RenameWorktreeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 90);

/**
 * Describes the message ket.remote.v1.DeleteWorktree.
 * Use `create(DeleteWorktreeSchema)` to create a new message.
 */
export const DeleteWorktreeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 91);

/**
 * Describes the message ket.remote.v1.WorktreeDeleted.
 * Use `create(WorktreeDeletedSchema)` to create a new message.
 */
export const WorktreeDeletedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 92);

/**
 * Describes the message ket.remote.v1.GetSnippets.
 * Use `create(GetSnippetsSchema)` to create a new message.
 */
export const GetSnippetsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 93);

/**
 * Describes the message ket.remote.v1.SnippetList.
 * Use `create(SnippetListSchema)` to create a new message.
 */
export const SnippetListSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 94);

/**
 * Describes the message ket.remote.v1.Snippet.
 * Use `create(SnippetSchema)` to create a new message.
 */
export const SnippetSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 95);

/**
 * Describes the message ket.remote.v1.ListDevices.
 * Use `create(ListDevicesSchema)` to create a new message.
 */
export const ListDevicesSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 96);

/**
 * Describes the message ket.remote.v1.DeviceList.
 * Use `create(DeviceListSchema)` to create a new message.
 */
export const DeviceListSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 97);

/**
 * Describes the message ket.remote.v1.Device.
 * Use `create(DeviceSchema)` to create a new message.
 */
export const DeviceSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 98);

/**
 * Describes the message ket.remote.v1.RevokeDevice.
 * Use `create(RevokeDeviceSchema)` to create a new message.
 */
export const RevokeDeviceSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 99);

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
 * Describes the enum ket.remote.v1.BacklogPriority.
 */
export const BacklogPrioritySchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 5);

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
  enumDesc(file_ket_remote_v1_remote, 6);

/**
 * Since 1.19. How GitHub lands a pull request.
 *
 * @generated from enum ket.remote.v1.PullRequestMergeMethod
 */
export const PullRequestMergeMethod = /*@__PURE__*/
  tsEnum(PullRequestMergeMethodSchema);

