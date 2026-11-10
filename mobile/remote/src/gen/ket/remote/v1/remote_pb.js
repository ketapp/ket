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

// @generated by protoc-gen-es v2.15.0 with parameter "target=js+dts,import_extension=js"
// @generated from file ket/remote/v1/remote.proto (package ket.remote.v1, syntax proto3)
/* eslint-disable */

import { enumDesc, fileDesc, messageDesc, tsEnum } from "@bufbuild/protobuf/codegenv2";

/**
 * Describes the file ket/remote/v1/remote.proto.
 */
export const file_ket_remote_v1_remote = /*@__PURE__*/
  fileDesc("ChprZXQvcmVtb3RlL3YxL3JlbW90ZS5wcm90bxINa2V0LnJlbW90ZS52MSKhHAoIRW52ZWxvcGUSDQoFbWFqb3IYASABKA0SDQoFbWlub3IYAiABKA0SEgoKcmVxdWVzdF9pZBgDIAEoBBIRCglzdHJlYW1faWQYBCABKAQSEgoKaG9zdF9lcG9jaBgFIAEoBBILCgNzZXEYBiABKAQSFwoPaWRlbXBvdGVuY3lfa2V5GAcgASgJEiUKBWhlbGxvGAogASgLMhQua2V0LnJlbW90ZS52MS5IZWxsb0gAEikKB3dlbGNvbWUYCyABKAsyFi5rZXQucmVtb3RlLnYxLldlbGNvbWVIABIlCgVlcnJvchgMIAEoCzIULmtldC5yZW1vdGUudjEuRXJyb3JIABIhCgNhY2sYDSABKAsyEi5rZXQucmVtb3RlLnYxLkFja0gAEjgKD3BhaXJpbmdfcGVuZGluZxgOIAEoCzIdLmtldC5yZW1vdGUudjEuUGFpcmluZ1BlbmRpbmdIABIvCghzbmFwc2hvdBgUIAEoCzIbLmtldC5yZW1vdGUudjEuSG9zdFNuYXBzaG90SAASLQoJc3Vic2NyaWJlGB4gASgLMhgua2V0LnJlbW90ZS52MS5TdWJzY3JpYmVIABIxCgt1bnN1YnNjcmliZRgfIAEoCzIaLmtldC5yZW1vdGUudjEuVW5zdWJzY3JpYmVIABI3CgpjaGVja3BvaW50GCAgASgLMiEua2V0LnJlbW90ZS52MS5UZXJtaW5hbENoZWNrcG9pbnRIABItCgVjaHVuaxghIAEoCzIcLmtldC5yZW1vdGUudjEuVGVybWluYWxDaHVua0gAEi0KBXJlc2V0GCIgASgLMhwua2V0LnJlbW90ZS52MS5UZXJtaW5hbFJlc2V0SAASLwoGY2xvc2VkGCMgASgLMh0ua2V0LnJlbW90ZS52MS5UZXJtaW5hbENsb3NlZEgAEiUKBWlucHV0GCggASgLMhQua2V0LnJlbW90ZS52MS5JbnB1dEgAEicKBnJlc2l6ZRgpIAEoCzIVLmtldC5yZW1vdGUudjEuUmVzaXplSAASJQoFbGVhc2UYKiABKAsyFC5rZXQucmVtb3RlLnYxLkxlYXNlSAASJwoGc2lnbmFsGCsgASgLMhUua2V0LnJlbW90ZS52MS5TaWduYWxIABIyCgxsaXN0X2RldmljZXMYMiABKAsyGi5rZXQucmVtb3RlLnYxLkxpc3REZXZpY2VzSAASLAoHZGV2aWNlcxgzIAEoCzIZLmtldC5yZW1vdGUudjEuRGV2aWNlTGlzdEgAEjQKDXJldm9rZV9kZXZpY2UYNCABKAsyGy5rZXQucmVtb3RlLnYxLlJldm9rZURldmljZUgAEicKBmFuc3dlchg8IAEoCzIVLmtldC5yZW1vdGUudjEuQW5zd2VySAASOgoQZ2V0X2NvbnZlcnNhdGlvbhhGIAEoCzIeLmtldC5yZW1vdGUudjEuR2V0Q29udmVyc2F0aW9uSAASMwoMY29udmVyc2F0aW9uGEcgASgLMhsua2V0LnJlbW90ZS52MS5Db252ZXJzYXRpb25IABIuCgpzdGFydF93b3JrGEggASgLMhgua2V0LnJlbW90ZS52MS5TdGFydFdvcmtIABIwCgtnZXRfY2hhbmdlcxhJIAEoCzIZLmtldC5yZW1vdGUudjEuR2V0Q2hhbmdlc0gAEikKB2NoYW5nZXMYSiABKAsyFi5rZXQucmVtb3RlLnYxLkNoYW5nZXNIABIqCghnZXRfZGlmZhhLIAEoCzIWLmtldC5yZW1vdGUudjEuR2V0RGlmZkgAEiMKBGRpZmYYTCABKAsyEy5rZXQucmVtb3RlLnYxLkRpZmZIABI2Cg5tZXJnZV93b3JrdHJlZRhNIAEoCzIcLmtldC5yZW1vdGUudjEuTWVyZ2VXb3JrdHJlZUgAEjIKDGdldF9zbmlwcGV0cxhOIAEoCzIaLmtldC5yZW1vdGUudjEuR2V0U25pcHBldHNIABIuCghzbmlwcGV0cxhPIAEoCzIaLmtldC5yZW1vdGUudjEuU25pcHBldExpc3RIABIpCgdyZXNwb25kGFAgASgLMhYua2V0LnJlbW90ZS52MS5SZXNwb25kSAASMAoLZ2V0X2JhY2tsb2cYWiABKAsyGS5rZXQucmVtb3RlLnYxLkdldEJhY2tsb2dIABIpCgdiYWNrbG9nGFsgASgLMhYua2V0LnJlbW90ZS52MS5CYWNrbG9nSAASMgoMZWRpdF9iYWNrbG9nGFwgASgLMhoua2V0LnJlbW90ZS52MS5FZGl0QmFja2xvZ0gAEi4KCnN0YXJ0X25vdGUYXSABKAsyGC5rZXQucmVtb3RlLnYxLlN0YXJ0Tm90ZUgAEi4KCmZpeF9jaGVja3MYZCABKAsyGC5rZXQucmVtb3RlLnYxLkZpeENoZWNrc0gAEj0KEmNoZWNrc19oYW5kZWRfb3ZlchhlIAEoCzIfLmtldC5yZW1vdGUudjEuQ2hlY2tzSGFuZGVkT3ZlckgAEi4KCnNlbmRfZmlsZXMYZiABKAsyGC5rZXQucmVtb3RlLnYxLlNlbmRGaWxlc0gAEjAKC3NlbmRfcmV2aWV3GGcgASgLMhkua2V0LnJlbW90ZS52MS5TZW5kUmV2aWV3SAASOwoRZ2V0X3N0YXJ0X29wdGlvbnMYaCABKAsyHi5rZXQucmVtb3RlLnYxLkdldFN0YXJ0T3B0aW9uc0gAEjQKDXN0YXJ0X29wdGlvbnMYaSABKAsyGy5rZXQucmVtb3RlLnYxLlN0YXJ0T3B0aW9uc0gAEjQKDXN0YXJ0X3Nlc3Npb24YaiABKAsyGy5rZXQucmVtb3RlLnYxLlN0YXJ0U2Vzc2lvbkgAEkQKFmdldF9wdWxsX3JlcXVlc3RfZHJhZnQYayABKAsyIi5rZXQucmVtb3RlLnYxLkdldFB1bGxSZXF1ZXN0RHJhZnRIABI9ChJwdWxsX3JlcXVlc3RfZHJhZnQYbCABKAsyHy5rZXQucmVtb3RlLnYxLlB1bGxSZXF1ZXN0RHJhZnRIABI7ChFvcGVuX3B1bGxfcmVxdWVzdBhtIAEoCzIeLmtldC5yZW1vdGUudjEuT3BlblB1bGxSZXF1ZXN0SAASPwoTcHVsbF9yZXF1ZXN0X29wZW5lZBhuIAEoCzIgLmtldC5yZW1vdGUudjEuUHVsbFJlcXVlc3RPcGVuZWRIABI9ChJtZXJnZV9wdWxsX3JlcXVlc3QYbyABKAsyHy5rZXQucmVtb3RlLnYxLk1lcmdlUHVsbFJlcXVlc3RIABI/ChNwdWxsX3JlcXVlc3RfbWVyZ2VkGHAgASgLMiAua2V0LnJlbW90ZS52MS5QdWxsUmVxdWVzdE1lcmdlZEgAEj8KE3VwZGF0ZV9wdWxsX3JlcXVlc3QYcSABKAsyIC5rZXQucmVtb3RlLnYxLlVwZGF0ZVB1bGxSZXF1ZXN0SAASQQoUcHVsbF9yZXF1ZXN0X3VwZGF0ZWQYciABKAsyIS5rZXQucmVtb3RlLnYxLlB1bGxSZXF1ZXN0VXBkYXRlZEgAEj0KEnJlYWR5X3B1bGxfcmVxdWVzdBhzIAEoCzIfLmtldC5yZW1vdGUudjEuUmVhZHlQdWxsUmVxdWVzdEgAEkEKFHB1bGxfcmVxdWVzdF9yZWFkaWVkGHQgASgLMiEua2V0LnJlbW90ZS52MS5QdWxsUmVxdWVzdFJlYWRpZWRIABI/ChNyZW9wZW5fcHVsbF9yZXF1ZXN0GHUgASgLMiAua2V0LnJlbW90ZS52MS5SZW9wZW5QdWxsUmVxdWVzdEgAEkMKFXB1bGxfcmVxdWVzdF9yZW9wZW5lZBh2IAEoCzIiLmtldC5yZW1vdGUudjEuUHVsbFJlcXVlc3RSZW9wZW5lZEgAEjoKEGFkZHJlc3NfY29tbWVudHMYdyABKAsyHi5rZXQucmVtb3RlLnYxLkFkZHJlc3NDb21tZW50c0gAEkEKFGNvbW1lbnRzX2hhbmRlZF9vdmVyGHggASgLMiEua2V0LnJlbW90ZS52MS5Db21tZW50c0hhbmRlZE92ZXJIABIyCgxwaW5fd29ya3RyZWUYeSABKAsyGi5rZXQucmVtb3RlLnYxLlBpbldvcmt0cmVlSAASOAoPZGVsZXRlX3dvcmt0cmVlGHogASgLMh0ua2V0LnJlbW90ZS52MS5EZWxldGVXb3JrdHJlZUgAEjoKEHdvcmt0cmVlX2RlbGV0ZWQYeyABKAsyHi5rZXQucmVtb3RlLnYxLldvcmt0cmVlRGVsZXRlZEgAEj0KEm9wZW5fYWdlbnRfc2Vzc2lvbhh8IAEoCzIfLmtldC5yZW1vdGUudjEuT3BlbkFnZW50U2Vzc2lvbkgAEjQKDXdha2VfdGVybWluYWwYfSABKAsyGy5rZXQucmVtb3RlLnYxLldha2VUZXJtaW5hbEgAEjYKDnRlcm1pbmFsX3dva2VuGH4gASgLMhwua2V0LnJlbW90ZS52MS5UZXJtaW5hbFdva2VuSAASKgoIc2V0X21vZGUYfyABKAsyFi5rZXQucmVtb3RlLnYxLlNldE1vZGVIABItCglzZXRfbW9kZWwYgAEgASgLMhcua2V0LnJlbW90ZS52MS5TZXRNb2RlbEgAEjEKC3N0YXJ0X2ZyZXNoGIEBIAEoCzIZLmtldC5yZW1vdGUudjEuU3RhcnRGcmVzaEgAEjUKDWZyZXNoX3N0YXJ0ZWQYggEgASgLMhsua2V0LnJlbW90ZS52MS5GcmVzaFN0YXJ0ZWRIAEIJCgdwYXlsb2FkIlsKBUhlbGxvEhMKC2RldmljZV9uYW1lGAEgASgJEhMKC2FwcF92ZXJzaW9uGAIgASgJEhQKDHJlc3VtZV9lcG9jaBgDIAEoBBISCgpyZXN1bWVfc2VxGAQgASgEIjcKDlBhaXJpbmdQZW5kaW5nEgwKBGNvZGUYASABKAkSFwoPdGltZW91dF9zZWNvbmRzGAIgASgNIkEKB1dlbGNvbWUSEQoJaG9zdF9uYW1lGAEgASgJEhIKCmhvc3RfZXBvY2gYAiABKAQSDwoHcmVzdW1lZBgDIAEoCCJACgVFcnJvchImCgRjb2RlGAEgASgOMhgua2V0LnJlbW90ZS52MS5FcnJvckNvZGUSDwoHbWVzc2FnZRgCIAEoCSIFCgNBY2siuAEKDEhvc3RTbmFwc2hvdBIvCghwcm9qZWN0cxgBIAMoCzIdLmtldC5yZW1vdGUudjEuUHJvamVjdFN1bW1hcnkSKwoFdXNhZ2UYAiADKAsyHC5rZXQucmVtb3RlLnYxLlByb3ZpZGVyVXNhZ2USDgoGYWdlbnRzGAMgAygJEicKB3BhbGV0dGUYBCABKAsyFi5rZXQucmVtb3RlLnYxLlBhbGV0dGUSEQoJYWRkcmVzc2VzGAUgAygJIqEDCgdQYWxldHRlEg0KBWxpZ2h0GAEgASgIEhAKCGJhY2tkcm9wGAIgASgJEg8KB3N1cmZhY2UYAyABKAkSDgoGc3Vua2VuGAQgASgJEhAKCGVsZXZhdGVkGAUgASgJEg0KBWhvdmVyGAYgASgJEhEKCXNlbGVjdGlvbhgHIAEoCRIOCgZib3JkZXIYCCABKAkSDAoEcnVsZRgJIAEoCRIOCgZhY2NlbnQYCiABKAkSEQoJb25fYWNjZW50GAsgASgJEgwKBHRleHQYDCABKAkSCwoDZGltGA0gASgJEg4KBm1hcmtlchgOIAEoCRIPCgdydW5uaW5nGA8gASgJEhEKCWF0dGVudGlvbhgQIAEoCRIOCgZmYWlsZWQYESABKAkSDwoHbWVyZ2luZxgSIAEoCRINCgVhZGRlZBgTIAEoCRIPCgdyZW1vdmVkGBQgASgJEhAKCG1vZGlmaWVkGBUgASgJEhIKCnF1b3RhX3dhcm0YFiABKAkSEQoJcXVvdGFfaG90GBcgASgJEhYKDnF1b3RhX2NyaXRpY2FsGBggASgJIq8BCg1Qcm92aWRlclVzYWdlEhAKCHByb3ZpZGVyGAEgASgJEioKBnN0YXR1cxgCIAEoDjIaLmtldC5yZW1vdGUudjEuVXNhZ2VTdGF0dXMSKwoHd2luZG93cxgDIAMoCzIaLmtldC5yZW1vdGUudjEuVXNhZ2VXaW5kb3cSDAoEcGxhbhgEIAEoCRIVCg1mZXRjaGVkX2F0X21zGAUgASgEEg4KBnJlYXNvbhgGIAEoCSJHCgtVc2FnZVdpbmRvdxIMCgRuYW1lGAEgASgJEhQKDHVzZWRfcGVyY2VudBgCIAEoAhIUCgxyZXNldHNfYXRfbXMYAyABKAQiiwEKDlByb2plY3RTdW1tYXJ5EgoKAmlkGAEgASgJEgwKBG5hbWUYAiABKAkSMQoJd29ya3RyZWVzGAMgAygLMh4ua2V0LnJlbW90ZS52MS5Xb3JrdHJlZVN1bW1hcnkSDQoFY29sb3IYBCABKAkSDwoHYmFja2xvZxgFIAEoDRIMCgRpY29uGAYgASgJIqcECg9Xb3JrdHJlZVN1bW1hcnkSCgoCaWQYASABKAkSDAoEbmFtZRgCIAEoCRIOCgZicmFuY2gYAyABKAkSEwoLYWdlbnRfc3RhdGUYBCABKAkSMQoJdGVybWluYWxzGAUgAygLMh4ua2V0LnJlbW90ZS52MS5UZXJtaW5hbFN1bW1hcnkSKQoIYWN0aXZpdHkYBiABKA4yFy5rZXQucmVtb3RlLnYxLkFjdGl2aXR5Eg0KBWFnZW50GAcgASgJEikKCHF1ZXN0aW9uGAggASgLMhcua2V0LnJlbW90ZS52MS5RdWVzdGlvbhIVCg1maWxlc19jaGFuZ2VkGAkgASgNEhMKC2xpbmVzX2FkZGVkGAogASgNEhUKDWxpbmVzX3JlbW92ZWQYCyABKA0SFQoNY29tbWl0c19haGVhZBgMIAEoDRIMCgRiYXNlGA0gASgJEjEKCXN1YmFnZW50cxgOIAMoCzIeLmtldC5yZW1vdGUudjEuU3ViYWdlbnRTdW1tYXJ5EjcKDHB1bGxfcmVxdWVzdBgPIAEoCzIhLmtldC5yZW1vdGUudjEuUHVsbFJlcXVlc3RTdW1tYXJ5EioKCXF1ZXN0aW9ucxgQIAMoCzIXLmtldC5yZW1vdGUudjEuUXVlc3Rpb24SDgoGcGlubmVkGBEgASgIEi0KBmFzbGVlcBgSIAMoCzIdLmtldC5yZW1vdGUudjEuQXNsZWVwVGVybWluYWwi+wMKElB1bGxSZXF1ZXN0U3VtbWFyeRIOCgZudW1iZXIYASABKAQSCwoDdXJsGAIgASgJEg0KBWRyYWZ0GAMgASgIEjgKBnJldmlldxgEIAEoDjIoLmtldC5yZW1vdGUudjEuUHVsbFJlcXVlc3RTdW1tYXJ5LlJldmlldxIrCgZjaGVja3MYBSADKAsyGy5rZXQucmVtb3RlLnYxLkNoZWNrU3VtbWFyeRI2CgVzdGF0ZRgGIAEoDjInLmtldC5yZW1vdGUudjEuUHVsbFJlcXVlc3RTdW1tYXJ5LlN0YXRlEgwKBGJhc2UYByABKAkSDgoGYmVoaW5kGAggASgIEhsKE3VucmVzb2x2ZWRfY29tbWVudHMYCSABKA0SHAoUbWVyZ2VfY29tbWl0X2FsbG93ZWQYCiABKAgSHAoUc3F1YXNoX21lcmdlX2FsbG93ZWQYCyABKAgSHAoUcmViYXNlX21lcmdlX2FsbG93ZWQYDCABKAgiWgoGUmV2aWV3EhYKElJFVklFV19VTlNQRUNJRklFRBAAEgwKCEFQUFJPVkVEEAESFQoRQ0hBTkdFU19SRVFVRVNURUQQAhITCg9SRVZJRVdfUkVRVUlSRUQQAyIpCgVTdGF0ZRIICgRPUEVOEAASCgoGQ0xPU0VEEAESCgoGTUVSR0VEEAIirQEKDENoZWNrU3VtbWFyeRIMCgRuYW1lGAEgASgJEjAKBXN0YXRlGAIgASgOMiEua2V0LnJlbW90ZS52MS5DaGVja1N1bW1hcnkuU3RhdGUSCwoDdXJsGAMgASgJIlAKBVN0YXRlEhUKEVNUQVRFX1VOU1BFQ0lGSUVEEAASCgoGUEFTU0VEEAESCgoGRkFJTEVEEAISCwoHUEVORElORxADEgsKB1NLSVBQRUQQBCLLAQoPU3ViYWdlbnRTdW1tYXJ5EgoKAmlkGAEgASgJEg0KBWxhYmVsGAIgASgJEg0KBW1vZGVsGAMgASgJEjMKBXN0YXRlGAQgASgOMiQua2V0LnJlbW90ZS52MS5TdWJhZ2VudFN1bW1hcnkuU3RhdGUSFQoNc3RhcnRlZF9hdF9tcxgFIAEoBCJCCgVTdGF0ZRIVChFTVEFURV9VTlNQRUNJRklFRBAAEgsKB1dPUktJTkcQARILCgdCTE9DS0VEEAISCAoESURMRRADIsMBCghRdWVzdGlvbhIMCgR0b29sGAEgASgJEg8KB3N1YmplY3QYAiABKAkSEgoKYW5zd2VyYWJsZRgDIAEoCBIOCgZhbHdheXMYBCABKAgSCgoCaWQYBSABKAQSEwoLdGVybWluYWxfaWQYBiABKAQSIQoEcmlzaxgHIAEoCzITLmtldC5yZW1vdGUudjEuUmlzaxIiCgRhc2tzGAggAygLMhQua2V0LnJlbW90ZS52MS5Bc2tlZBIMCgRwbGFuGAkgASgIIpsBCgVBc2tlZBIQCghxdWVzdGlvbhgBIAEoCRIOCgZoZWFkZXIYAiABKAkSLAoHY2hvaWNlcxgDIAMoCzIbLmtldC5yZW1vdGUudjEuQXNrZWQuQ2hvaWNlEhQKDG11bHRpX3NlbGVjdBgEIAEoCBosCgZDaG9pY2USDQoFbGFiZWwYASABKAkSEwoLZGVzY3JpcHRpb24YAiABKAkiKQoEUmlzaxINCgVsZXZlbBgBIAEoCRISCgpjb25maWRlbmNlGAIgASgCImIKDkFzbGVlcFRlcm1pbmFsEgsKA2tleRgBIAEoCRINCgV0aXRsZRgCIAEoCRINCgVhZ2VudBgDIAEoCRIQCghzaW5jZV9tcxgEIAEoBBITCgtmcmVlZF9ieXRlcxgFIAEoBCIwCgxXYWtlVGVybWluYWwSEwoLd29ya3RyZWVfaWQYASABKAkSCwoDa2V5GAIgASgJIiQKDVRlcm1pbmFsV29rZW4SEwoLdGVybWluYWxfaWQYASABKAQi5QIKD1Rlcm1pbmFsU3VtbWFyeRIKCgJpZBgBIAEoBBINCgV0aXRsZRgCIAEoCRINCgVhZ2VudBgDIAEoCRIOCgZjbG9zZWQYBCABKAgSLAoKYWdlbnRfdmlldxgFIAEoCzIYLmtldC5yZW1vdGUudjEuQWdlbnRWaWV3EigKBW1vZGVzGAYgASgLMhkua2V0LnJlbW90ZS52MS5BZ2VudE1vZGVzEi4KCHNldHRpbmdzGAcgASgLMhwua2V0LnJlbW90ZS52MS5BZ2VudFNldHRpbmdzEiYKBGNvbGQYCCABKAsyGC5rZXQucmVtb3RlLnYxLkNvbGRDYWNoZRIpCghhY3Rpdml0eRgJIAEoDjIXLmtldC5yZW1vdGUudjEuQWN0aXZpdHkSDwoHcnVubmluZxgKIAEoCRIYChBydW5uaW5nX3NpbmNlX21zGAsgASgEEhIKCmNvbXBhY3RpbmcYDCABKAgiZAoJQ29sZENhY2hlEg8KB3Nlc3Npb24YASABKAkSEAoIc2luY2VfbXMYAiABKAQSDgoGdG9rZW5zGAMgASgEEhMKC2Nvc3RfbWljcm9zGAQgASgDEg8KB3N1bW1hcnkYBSABKAkiRAoKU3RhcnRGcmVzaBITCgt0ZXJtaW5hbF9pZBgBIAEoBBIPCgdzZXNzaW9uGAIgASgJEhAKCHNpbmNlX21zGAMgASgEIiMKDEZyZXNoU3RhcnRlZBITCgt0ZXJtaW5hbF9pZBgBIAEoBCJICgpBZ2VudE1vZGVzEg8KB2N1cnJlbnQYASABKAkSKQoHY2hvaWNlcxgCIAMoCzIYLmtldC5yZW1vdGUudjEuQWdlbnRNb2RlIjYKCUFnZW50TW9kZRIKCgJpZBgBIAEoCRINCgVsYWJlbBgCIAEoCRIOCgZkZXRhaWwYAyABKAkiLAoHU2V0TW9kZRITCgt0ZXJtaW5hbF9pZBgBIAEoBBIMCgRtb2RlGAIgASgJIp4BCg1BZ2VudFNldHRpbmdzEg0KBW1vZGVsGAEgASgJEg4KBmVmZm9ydBgCIAEoCRIqCgZtb2RlbHMYAyADKAsyGi5rZXQucmVtb3RlLnYxLkFnZW50Q2hvaWNlEisKB2VmZm9ydHMYBCADKAsyGi5rZXQucmVtb3RlLnYxLkFnZW50Q2hvaWNlEhUKDXNhdmVzX2RlZmF1bHQYBSABKAgiKAoLQWdlbnRDaG9pY2USCgoCaWQYASABKAkSDQoFbGFiZWwYAiABKAkiPgoIU2V0TW9kZWwSEwoLdGVybWluYWxfaWQYASABKAQSDQoFbW9kZWwYAiABKAkSDgoGZWZmb3J0GAMgASgJIk8KCUFnZW50VmlldxIyCghzZXNzaW9ucxgBIAMoCzIgLmtldC5yZW1vdGUudjEuQmFja2dyb3VuZFNlc3Npb24SDgoGbGlzdGVkGAIgASgIIusBChFCYWNrZ3JvdW5kU2Vzc2lvbhIPCgdzZXNzaW9uGAEgASgJEgwKBG5hbWUYAiABKAkSNQoFc3RhdGUYAyABKA4yJi5rZXQucmVtb3RlLnYxLkJhY2tncm91bmRTZXNzaW9uLlN0YXRlEhUKDXN0YXJ0ZWRfYXRfbXMYBCABKAMSFAoMYWN0aXZlX2F0X21zGAUgASgDEg0KBWRvaW5nGAYgASgJIkQKBVN0YXRlEhUKEVNUQVRFX1VOU1BFQ0lGSUVEEAASCAoERE9ORRABEgsKB1dPUktJTkcQAhINCglORUVEU19ZT1UQAyIgCglTdWJzY3JpYmUSEwoLdGVybWluYWxfaWQYASABKAQiIgoLVW5zdWJzY3JpYmUSEwoLdGVybWluYWxfaWQYASABKAQiiwEKElRlcm1pbmFsQ2hlY2twb2ludBITCgt0ZXJtaW5hbF9pZBgBIAEoBBINCgVlcG9jaBgCIAEoBBITCgt0aHJvdWdoX3NlcRgDIAEoBBIMCgRjb2xzGAQgASgNEgwKBHJvd3MYBSABKA0SEgoKc2Nyb2xsYmFjaxgGIAEoDRIMCgRhbnNpGAcgASgMIk8KDVRlcm1pbmFsQ2h1bmsSEwoLdGVybWluYWxfaWQYASABKAQSDQoFZXBvY2gYAiABKAQSCwoDc2VxGAMgASgEEg0KBWJ5dGVzGAQgASgMIiQKDVRlcm1pbmFsUmVzZXQSEwoLdGVybWluYWxfaWQYASABKAQiWwoOVGVybWluYWxDbG9zZWQSEwoLdGVybWluYWxfaWQYASABKAQSFgoJZXhpdF9jb2RlGAIgASgNSACIAQESDgoGc2lnbmFsGAMgASgJQgwKCl9leGl0X2NvZGUiKwoFSW5wdXQSEwoLdGVybWluYWxfaWQYASABKAQSDQoFYnl0ZXMYAiABKAwiXgoJU2VuZEZpbGVzEhMKC3Rlcm1pbmFsX2lkGAEgASgEEgwKBHRleHQYAiABKAkSLgoLYXR0YWNobWVudHMYAyADKAsyGS5rZXQucmVtb3RlLnYxLkF0dGFjaG1lbnQiKAoKQXR0YWNobWVudBIMCgRuYW1lGAEgASgJEgwKBGRhdGEYAiABKAwiOQoGUmVzaXplEhMKC3Rlcm1pbmFsX2lkGAEgASgEEgwKBGNvbHMYAiABKA0SDAoEcm93cxgDIAEoDSKQAQoFTGVhc2USEwoLdGVybWluYWxfaWQYASABKAQSKwoGYWN0aW9uGAIgASgOMhsua2V0LnJlbW90ZS52MS5MZWFzZS5BY3Rpb24iRQoGQWN0aW9uEhYKEkFDVElPTl9VTlNQRUNJRklFRBAAEgsKB0FDUVVJUkUQARIJCgVSRU5FVxACEgsKB1JFTEVBU0UQAyKDAQoGU2lnbmFsEhMKC3Rlcm1pbmFsX2lkGAEgASgEEigKBGtpbmQYAiABKA4yGi5rZXQucmVtb3RlLnYxLlNpZ25hbC5LaW5kIjoKBEtpbmQSFAoQS0lORF9VTlNQRUNJRklFRBAAEg0KCUlOVEVSUlVQVBABEg0KCVRFUk1JTkFURRACIq4BCgZBbnN3ZXISEwoLdGVybWluYWxfaWQYASABKAQSLAoGY2hvaWNlGAIgASgOMhwua2V0LnJlbW90ZS52MS5BbnN3ZXIuQ2hvaWNlEhMKC3F1ZXN0aW9uX2lkGAMgASgEIkwKBkNob2ljZRIWChJDSE9JQ0VfVU5TUEVDSUZJRUQQABIOCgpBTExPV19PTkNFEAESEAoMQUxMT1dfQUxXQVlTEAISCAoEREVOWRADIqoBCgdSZXNwb25kEhMKC3Rlcm1pbmFsX2lkGAEgASgEEhMKC3F1ZXN0aW9uX2lkGAIgASgEEioKBXBpY2tzGAMgAygLMhsua2V0LnJlbW90ZS52MS5SZXNwb25kLlBpY2sSDwoHYXBwcm92ZRgEIAEoCBIQCghmZWVkYmFjaxgFIAEoCRomCgRQaWNrEg8KB2Nob2ljZXMYASADKA0SDQoFb3RoZXIYAiABKAkiRgoPR2V0Q29udmVyc2F0aW9uEhMKC3Rlcm1pbmFsX2lkGAEgASgEEg0KBWFmdGVyGAIgASgEEg8KB3Nlc3Npb24YAyABKAkiqAEKDENvbnZlcnNhdGlvbhITCgt0ZXJtaW5hbF9pZBgBIAEoBBIRCglzdXBwb3J0ZWQYAiABKAgSDgoGcmVhc29uGAMgASgJEg8KB3Nlc3Npb24YBCABKAkSIgoFdHVybnMYBSADKAsyEy5rZXQucmVtb3RlLnYxLlR1cm4SDAoEbmV4dBgGIAEoBBINCgVmcmVzaBgHIAEoCBIOCgZxdWV1ZWQYCCADKAkimwEKBFR1cm4SJgoEa2luZBgBIAEoDjIYLmtldC5yZW1vdGUudjEuVHVybi5LaW5kEgwKBHRleHQYAiABKAkSDAoEdG9vbBgDIAEoCRIOCgZpbWFnZXMYBCABKA0iPwoES2luZBIUChBLSU5EX1VOU1BFQ0lGSUVEEAASCAoEVVNFUhABEg0KCUFTU0lTVEFOVBACEggKBFRPT0wQAyJrCglTdGFydFdvcmsSEgoKcHJvamVjdF9pZBgBIAEoCRIOCgZwcm9tcHQYAiABKAkSDQoFYWdlbnQYAyABKAkSDAoEYmFzZRgEIAEoCRINCgVtb2RlbBgFIAEoCRIOCgZlZmZvcnQYBiABKAkiJQoPR2V0U3RhcnRPcHRpb25zEhIKCnByb2plY3RfaWQYASABKAkiQwoRQWdlbnRTdGFydE9wdGlvbnMSDQoFYWdlbnQYASABKAkSDgoGbW9kZWxzGAIgAygJEg8KB2VmZm9ydHMYAyADKAkiggEKDFN0YXJ0T3B0aW9ucxISCgpwcm9qZWN0X2lkGAEgASgJEhQKDGRlZmF1bHRfYmFzZRgCIAEoCRIWCg5sb2NhbF9icmFuY2hlcxgDIAMoCRIwCgZhZ2VudHMYBCADKAsyIC5rZXQucmVtb3RlLnYxLkFnZW50U3RhcnRPcHRpb25zImEKDFN0YXJ0U2Vzc2lvbhITCgt3b3JrdHJlZV9pZBgBIAEoCRINCgVhZ2VudBgCIAEoCRIOCgZwcm9tcHQYAyABKAkSDQoFbW9kZWwYBCABKAkSDgoGZWZmb3J0GAUgASgJIiAKCkdldEJhY2tsb2cSEgoKcHJvamVjdF9pZBgBIAEoCSJICgdCYWNrbG9nEhIKCnByb2plY3RfaWQYASABKAkSKQoFbm90ZXMYAiADKAsyGi5rZXQucmVtb3RlLnYxLkJhY2tsb2dOb3RlIsABCgtCYWNrbG9nTm90ZRIKCgJpZBgBIAEoCRINCgV0aXRsZRgCIAEoCRIMCgRib2R5GAMgASgJEg0KBWZpbGVzGAQgAygJEhIKCmNyZWF0ZWRfbXMYBSABKAQSEgoKdXBkYXRlZF9tcxgGIAEoBBIPCgdkb25lX21zGAcgASgEEg4KBmJyYW5jaBgIIAEoCRIwCghwcmlvcml0eRgJIAEoDjIeLmtldC5yZW1vdGUudjEuQmFja2xvZ1ByaW9yaXR5IoICCgtFZGl0QmFja2xvZxISCgpwcm9qZWN0X2lkGAEgASgJEg8KB25vdGVfaWQYAiABKAkSMQoGYWN0aW9uGAMgASgOMiEua2V0LnJlbW90ZS52MS5FZGl0QmFja2xvZy5BY3Rpb24SDQoFdGl0bGUYBCABKAkSDAoEYm9keRgFIAEoCRIwCghwcmlvcml0eRgGIAEoDjIeLmtldC5yZW1vdGUudjEuQmFja2xvZ1ByaW9yaXR5IkwKBkFjdGlvbhIWChJBQ1RJT05fVU5TUEVDSUZJRUQQABIICgRTQVZFEAESCAoERE9ORRACEgoKBlJFT1BFThADEgoKBlJFTU9WRRAEIjAKCVN0YXJ0Tm90ZRISCgpwcm9qZWN0X2lkGAEgASgJEg8KB25vdGVfaWQYAiABKAkiIQoKR2V0Q2hhbmdlcxITCgt3b3JrdHJlZV9pZBgBIAEoCSJlCgdDaGFuZ2VzEhMKC3dvcmt0cmVlX2lkGAEgASgJEigKBWZpbGVzGAIgAygLMhkua2V0LnJlbW90ZS52MS5GaWxlQ2hhbmdlEgwKBGJhc2UYAyABKAkSDQoFZXJyb3IYBCABKAkixAEKCkZpbGVDaGFuZ2USDAoEcGF0aBgBIAEoCRINCgVhZGRlZBgCIAEoDRIPCgdyZW1vdmVkGAMgASgNEjAKBnN0YXR1cxgEIAEoDjIgLmtldC5yZW1vdGUudjEuRmlsZUNoYW5nZS5TdGF0dXMSDgoGYmluYXJ5GAUgASgIIkYKBlN0YXR1cxIWChJTVEFUVVNfVU5TUEVDSUZJRUQQABIMCghNT0RJRklFRBABEgkKBUFEREVEEAISCwoHREVMRVRFRBADIiwKB0dldERpZmYSEwoLd29ya3RyZWVfaWQYASABKAkSDAoEcGF0aBgCIAEoCSJZCgREaWZmEhMKC3dvcmt0cmVlX2lkGAEgASgJEgwKBHBhdGgYAiABKAkSDAoEdGV4dBgDIAEoCRIRCgl0cnVuY2F0ZWQYBCABKAgSDQoFZXJyb3IYBSABKAkiJAoNTWVyZ2VXb3JrdHJlZRITCgt3b3JrdHJlZV9pZBgBIAEoCSIgCglGaXhDaGVja3MSEwoLd29ya3RyZWVfaWQYASABKAkiJAoQQ2hlY2tzSGFuZGVkT3ZlchIQCghzdGFydGluZxgBIAEoCCJRCgpTZW5kUmV2aWV3EhMKC3dvcmt0cmVlX2lkGAEgASgJEi4KCGNvbW1lbnRzGAIgAygLMhwua2V0LnJlbW90ZS52MS5SZXZpZXdDb21tZW50IksKDVJldmlld0NvbW1lbnQSDAoEcGF0aBgBIAEoCRINCgVsaW5lcxgCIAEoCRIMCgR0ZXh0GAMgASgJEg8KB2NvbW1lbnQYBCABKAkiKgoTR2V0UHVsbFJlcXVlc3REcmFmdBITCgt3b3JrdHJlZV9pZBgBIAEoCSKyAQoQUHVsbFJlcXVlc3REcmFmdBITCgt3b3JrdHJlZV9pZBgBIAEoCRIOCgZicmFuY2gYAiABKAkSDAoEYmFzZRgDIAEoCRIOCgZyZW1vdGUYBCABKAkSDQoFdGl0bGUYBSABKAkSDAoEYm9keRgGIAEoCRIPCgdjaGFuZ2VzGAcgASgNEhcKD2V4aXN0aW5nX251bWJlchgIIAEoBBIUCgxleGlzdGluZ191cmwYCSABKAkiUgoPT3BlblB1bGxSZXF1ZXN0EhMKC3dvcmt0cmVlX2lkGAEgASgJEg0KBXRpdGxlGAIgASgJEgwKBGJvZHkYAyABKAkSDQoFZHJhZnQYBCABKAgiggEKEVB1bGxSZXF1ZXN0T3BlbmVkEg4KBm51bWJlchgBIAEoBBILCgN1cmwYAiABKAkSDgoGYnJhbmNoGAMgASgJEgwKBGJhc2UYBCABKAkSDgoGcmVtb3RlGAUgASgJEg8KB2V4aXN0ZWQYBiABKAgSEQoJY29tbWl0dGVkGAcgASgNIl4KEE1lcmdlUHVsbFJlcXVlc3QSEwoLd29ya3RyZWVfaWQYASABKAkSNQoGbWV0aG9kGAIgASgOMiUua2V0LnJlbW90ZS52MS5QdWxsUmVxdWVzdE1lcmdlTWV0aG9kImcKEVB1bGxSZXF1ZXN0TWVyZ2VkEg4KBm51bWJlchgBIAEoBBILCgN1cmwYAiABKAkSNQoGbWV0aG9kGAMgASgOMiUua2V0LnJlbW90ZS52MS5QdWxsUmVxdWVzdE1lcmdlTWV0aG9kIigKEVVwZGF0ZVB1bGxSZXF1ZXN0EhMKC3dvcmt0cmVlX2lkGAEgASgJIiQKElB1bGxSZXF1ZXN0VXBkYXRlZBIOCgZudW1iZXIYASABKAQiJwoQUmVhZHlQdWxsUmVxdWVzdBITCgt3b3JrdHJlZV9pZBgBIAEoCSIkChJQdWxsUmVxdWVzdFJlYWRpZWQSDgoGbnVtYmVyGAEgASgEIigKEVJlb3BlblB1bGxSZXF1ZXN0EhMKC3dvcmt0cmVlX2lkGAEgASgJIiUKE1B1bGxSZXF1ZXN0UmVvcGVuZWQSDgoGbnVtYmVyGAEgASgEIiYKD0FkZHJlc3NDb21tZW50cxITCgt3b3JrdHJlZV9pZBgBIAEoCSImChJDb21tZW50c0hhbmRlZE92ZXISEAoIc3RhcnRpbmcYASABKAgiRgoQT3BlbkFnZW50U2Vzc2lvbhITCgt0ZXJtaW5hbF9pZBgBIAEoBBIPCgdzZXNzaW9uGAIgASgJEgwKBHRleHQYAyABKAkiMgoLUGluV29ya3RyZWUSEwoLd29ya3RyZWVfaWQYASABKAkSDgoGcGlubmVkGAIgASgIIjQKDkRlbGV0ZVdvcmt0cmVlEhMKC3dvcmt0cmVlX2lkGAEgASgJEg0KBWZvcmNlGAIgASgIIiYKD1dvcmt0cmVlRGVsZXRlZBITCgtrZXB0X2JyYW5jaBgBIAEoCSINCgtHZXRTbmlwcGV0cyI3CgtTbmlwcGV0TGlzdBIoCghzbmlwcGV0cxgBIAMoCzIWLmtldC5yZW1vdGUudjEuU25pcHBldCIlCgdTbmlwcGV0EgwKBG5hbWUYASABKAkSDAoEYm9keRgCIAEoCSINCgtMaXN0RGV2aWNlcyI0CgpEZXZpY2VMaXN0EiYKB2RldmljZXMYASADKAsyFS5rZXQucmVtb3RlLnYxLkRldmljZSJdCgZEZXZpY2USCgoCaWQYASABKAkSDAoEbmFtZRgCIAEoCRIRCglwYWlyZWRfYXQYAyABKAQSEwoLdGhpc19kZXZpY2UYBCABKAgSEQoJY29ubmVjdGVkGAUgASgIIhoKDFJldm9rZURldmljZRIKCgJpZBgBIAEoCSq9AQoJRXJyb3JDb2RlEhoKFkVSUk9SX0NPREVfVU5TUEVDSUZJRUQQABIQCgxVTkFVVEhPUklaRUQQARIRCg1TVEFMRV9SRVFVRVNUEAISDQoJTk9UX0ZPVU5EEAMSEgoOTk9UX0NPTlRST0xMRVIQBBIQCgxCQUNLUFJFU1NVUkUQBRIXChNVTlNVUFBPUlRFRF9WRVJTSU9OEAYSDAoIREVDTElORUQQBxITCg9XT1VMRF9MT1NFX1dPUksQCCqXAQoLVXNhZ2VTdGF0dXMSHAoYVVNBR0VfU1RBVFVTX1VOU1BFQ0lGSUVEEAASFgoSVVNBR0VfU1RBVFVTX0ZSRVNIEAESFgoSVVNBR0VfU1RBVFVTX1NUQUxFEAISHAoYVVNBR0VfU1RBVFVTX1VOQVZBSUxBQkxFEAMSHAoYVVNBR0VfU1RBVFVTX1VOU1VQUE9SVEVEEAQqiwEKCEFjdGl2aXR5EhIKDkFDVElWSVRZX1FVSUVUEAASFAoQQUNUSVZJVFlfUlVOTklORxABEhQKEEFDVElWSVRZX1dPUktJTkcQAhIUChBBQ1RJVklUWV9NRVJHSU5HEAMSFAoQQUNUSVZJVFlfQkxPQ0tFRBAEEhMKD0FDVElWSVRZX0ZBSUxFRBAFKqIBCg9CYWNrbG9nUHJpb3JpdHkSIAocQkFDS0xPR19QUklPUklUWV9VTlNQRUNJRklFRBAAEhgKFEJBQ0tMT0dfUFJJT1JJVFlfTE9XEAESGwoXQkFDS0xPR19QUklPUklUWV9NRURJVU0QAhIZChVCQUNLTE9HX1BSSU9SSVRZX0hJR0gQAxIbChdCQUNLTE9HX1BSSU9SSVRZX1VSR0VOVBAEKm0KFlB1bGxSZXF1ZXN0TWVyZ2VNZXRob2QSKQolUFVMTF9SRVFVRVNUX01FUkdFX01FVEhPRF9VTlNQRUNJRklFRBAAEhAKDE1FUkdFX0NPTU1JVBABEgoKBlNRVUFTSBACEgoKBlJFQkFTRRADYgZwcm90bzM");

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
 * Describes the message ket.remote.v1.ColdCache.
 * Use `create(ColdCacheSchema)` to create a new message.
 */
export const ColdCacheSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 22);

/**
 * Describes the message ket.remote.v1.StartFresh.
 * Use `create(StartFreshSchema)` to create a new message.
 */
export const StartFreshSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 23);

/**
 * Describes the message ket.remote.v1.FreshStarted.
 * Use `create(FreshStartedSchema)` to create a new message.
 */
export const FreshStartedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 24);

/**
 * Describes the message ket.remote.v1.AgentModes.
 * Use `create(AgentModesSchema)` to create a new message.
 */
export const AgentModesSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 25);

/**
 * Describes the message ket.remote.v1.AgentMode.
 * Use `create(AgentModeSchema)` to create a new message.
 */
export const AgentModeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 26);

/**
 * Describes the message ket.remote.v1.SetMode.
 * Use `create(SetModeSchema)` to create a new message.
 */
export const SetModeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 27);

/**
 * Describes the message ket.remote.v1.AgentSettings.
 * Use `create(AgentSettingsSchema)` to create a new message.
 */
export const AgentSettingsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 28);

/**
 * Describes the message ket.remote.v1.AgentChoice.
 * Use `create(AgentChoiceSchema)` to create a new message.
 */
export const AgentChoiceSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 29);

/**
 * Describes the message ket.remote.v1.SetModel.
 * Use `create(SetModelSchema)` to create a new message.
 */
export const SetModelSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 30);

/**
 * Describes the message ket.remote.v1.AgentView.
 * Use `create(AgentViewSchema)` to create a new message.
 */
export const AgentViewSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 31);

/**
 * Describes the message ket.remote.v1.BackgroundSession.
 * Use `create(BackgroundSessionSchema)` to create a new message.
 */
export const BackgroundSessionSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 32);

/**
 * Describes the enum ket.remote.v1.BackgroundSession.State.
 */
export const BackgroundSession_StateSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 32, 0);

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
  messageDesc(file_ket_remote_v1_remote, 33);

/**
 * Describes the message ket.remote.v1.Unsubscribe.
 * Use `create(UnsubscribeSchema)` to create a new message.
 */
export const UnsubscribeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 34);

/**
 * Describes the message ket.remote.v1.TerminalCheckpoint.
 * Use `create(TerminalCheckpointSchema)` to create a new message.
 */
export const TerminalCheckpointSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 35);

/**
 * Describes the message ket.remote.v1.TerminalChunk.
 * Use `create(TerminalChunkSchema)` to create a new message.
 */
export const TerminalChunkSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 36);

/**
 * Describes the message ket.remote.v1.TerminalReset.
 * Use `create(TerminalResetSchema)` to create a new message.
 */
export const TerminalResetSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 37);

/**
 * Describes the message ket.remote.v1.TerminalClosed.
 * Use `create(TerminalClosedSchema)` to create a new message.
 */
export const TerminalClosedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 38);

/**
 * Describes the message ket.remote.v1.Input.
 * Use `create(InputSchema)` to create a new message.
 */
export const InputSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 39);

/**
 * Describes the message ket.remote.v1.SendFiles.
 * Use `create(SendFilesSchema)` to create a new message.
 */
export const SendFilesSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 40);

/**
 * Describes the message ket.remote.v1.Attachment.
 * Use `create(AttachmentSchema)` to create a new message.
 */
export const AttachmentSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 41);

/**
 * Describes the message ket.remote.v1.Resize.
 * Use `create(ResizeSchema)` to create a new message.
 */
export const ResizeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 42);

/**
 * Describes the message ket.remote.v1.Lease.
 * Use `create(LeaseSchema)` to create a new message.
 */
export const LeaseSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 43);

/**
 * Describes the enum ket.remote.v1.Lease.Action.
 */
export const Lease_ActionSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 43, 0);

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
  messageDesc(file_ket_remote_v1_remote, 44);

/**
 * Describes the enum ket.remote.v1.Signal.Kind.
 */
export const Signal_KindSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 44, 0);

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
  messageDesc(file_ket_remote_v1_remote, 45);

/**
 * Describes the enum ket.remote.v1.Answer.Choice.
 */
export const Answer_ChoiceSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 45, 0);

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
  messageDesc(file_ket_remote_v1_remote, 46);

/**
 * Describes the message ket.remote.v1.Respond.Pick.
 * Use `create(Respond_PickSchema)` to create a new message.
 */
export const Respond_PickSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 46, 0);

/**
 * Describes the message ket.remote.v1.GetConversation.
 * Use `create(GetConversationSchema)` to create a new message.
 */
export const GetConversationSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 47);

/**
 * Describes the message ket.remote.v1.Conversation.
 * Use `create(ConversationSchema)` to create a new message.
 */
export const ConversationSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 48);

/**
 * Describes the message ket.remote.v1.Turn.
 * Use `create(TurnSchema)` to create a new message.
 */
export const TurnSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 49);

/**
 * Describes the enum ket.remote.v1.Turn.Kind.
 */
export const Turn_KindSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 49, 0);

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
  messageDesc(file_ket_remote_v1_remote, 50);

/**
 * Describes the message ket.remote.v1.GetStartOptions.
 * Use `create(GetStartOptionsSchema)` to create a new message.
 */
export const GetStartOptionsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 51);

/**
 * Describes the message ket.remote.v1.AgentStartOptions.
 * Use `create(AgentStartOptionsSchema)` to create a new message.
 */
export const AgentStartOptionsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 52);

/**
 * Describes the message ket.remote.v1.StartOptions.
 * Use `create(StartOptionsSchema)` to create a new message.
 */
export const StartOptionsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 53);

/**
 * Describes the message ket.remote.v1.StartSession.
 * Use `create(StartSessionSchema)` to create a new message.
 */
export const StartSessionSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 54);

/**
 * Describes the message ket.remote.v1.GetBacklog.
 * Use `create(GetBacklogSchema)` to create a new message.
 */
export const GetBacklogSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 55);

/**
 * Describes the message ket.remote.v1.Backlog.
 * Use `create(BacklogSchema)` to create a new message.
 */
export const BacklogSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 56);

/**
 * Describes the message ket.remote.v1.BacklogNote.
 * Use `create(BacklogNoteSchema)` to create a new message.
 */
export const BacklogNoteSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 57);

/**
 * Describes the message ket.remote.v1.EditBacklog.
 * Use `create(EditBacklogSchema)` to create a new message.
 */
export const EditBacklogSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 58);

/**
 * Describes the enum ket.remote.v1.EditBacklog.Action.
 */
export const EditBacklog_ActionSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 58, 0);

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
  messageDesc(file_ket_remote_v1_remote, 59);

/**
 * Describes the message ket.remote.v1.GetChanges.
 * Use `create(GetChangesSchema)` to create a new message.
 */
export const GetChangesSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 60);

/**
 * Describes the message ket.remote.v1.Changes.
 * Use `create(ChangesSchema)` to create a new message.
 */
export const ChangesSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 61);

/**
 * Describes the message ket.remote.v1.FileChange.
 * Use `create(FileChangeSchema)` to create a new message.
 */
export const FileChangeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 62);

/**
 * Describes the enum ket.remote.v1.FileChange.Status.
 */
export const FileChange_StatusSchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 62, 0);

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
  messageDesc(file_ket_remote_v1_remote, 63);

/**
 * Describes the message ket.remote.v1.Diff.
 * Use `create(DiffSchema)` to create a new message.
 */
export const DiffSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 64);

/**
 * Describes the message ket.remote.v1.MergeWorktree.
 * Use `create(MergeWorktreeSchema)` to create a new message.
 */
export const MergeWorktreeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 65);

/**
 * Describes the message ket.remote.v1.FixChecks.
 * Use `create(FixChecksSchema)` to create a new message.
 */
export const FixChecksSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 66);

/**
 * Describes the message ket.remote.v1.ChecksHandedOver.
 * Use `create(ChecksHandedOverSchema)` to create a new message.
 */
export const ChecksHandedOverSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 67);

/**
 * Describes the message ket.remote.v1.SendReview.
 * Use `create(SendReviewSchema)` to create a new message.
 */
export const SendReviewSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 68);

/**
 * Describes the message ket.remote.v1.ReviewComment.
 * Use `create(ReviewCommentSchema)` to create a new message.
 */
export const ReviewCommentSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 69);

/**
 * Describes the message ket.remote.v1.GetPullRequestDraft.
 * Use `create(GetPullRequestDraftSchema)` to create a new message.
 */
export const GetPullRequestDraftSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 70);

/**
 * Describes the message ket.remote.v1.PullRequestDraft.
 * Use `create(PullRequestDraftSchema)` to create a new message.
 */
export const PullRequestDraftSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 71);

/**
 * Describes the message ket.remote.v1.OpenPullRequest.
 * Use `create(OpenPullRequestSchema)` to create a new message.
 */
export const OpenPullRequestSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 72);

/**
 * Describes the message ket.remote.v1.PullRequestOpened.
 * Use `create(PullRequestOpenedSchema)` to create a new message.
 */
export const PullRequestOpenedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 73);

/**
 * Describes the message ket.remote.v1.MergePullRequest.
 * Use `create(MergePullRequestSchema)` to create a new message.
 */
export const MergePullRequestSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 74);

/**
 * Describes the message ket.remote.v1.PullRequestMerged.
 * Use `create(PullRequestMergedSchema)` to create a new message.
 */
export const PullRequestMergedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 75);

/**
 * Describes the message ket.remote.v1.UpdatePullRequest.
 * Use `create(UpdatePullRequestSchema)` to create a new message.
 */
export const UpdatePullRequestSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 76);

/**
 * Describes the message ket.remote.v1.PullRequestUpdated.
 * Use `create(PullRequestUpdatedSchema)` to create a new message.
 */
export const PullRequestUpdatedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 77);

/**
 * Describes the message ket.remote.v1.ReadyPullRequest.
 * Use `create(ReadyPullRequestSchema)` to create a new message.
 */
export const ReadyPullRequestSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 78);

/**
 * Describes the message ket.remote.v1.PullRequestReadied.
 * Use `create(PullRequestReadiedSchema)` to create a new message.
 */
export const PullRequestReadiedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 79);

/**
 * Describes the message ket.remote.v1.ReopenPullRequest.
 * Use `create(ReopenPullRequestSchema)` to create a new message.
 */
export const ReopenPullRequestSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 80);

/**
 * Describes the message ket.remote.v1.PullRequestReopened.
 * Use `create(PullRequestReopenedSchema)` to create a new message.
 */
export const PullRequestReopenedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 81);

/**
 * Describes the message ket.remote.v1.AddressComments.
 * Use `create(AddressCommentsSchema)` to create a new message.
 */
export const AddressCommentsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 82);

/**
 * Describes the message ket.remote.v1.CommentsHandedOver.
 * Use `create(CommentsHandedOverSchema)` to create a new message.
 */
export const CommentsHandedOverSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 83);

/**
 * Describes the message ket.remote.v1.OpenAgentSession.
 * Use `create(OpenAgentSessionSchema)` to create a new message.
 */
export const OpenAgentSessionSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 84);

/**
 * Describes the message ket.remote.v1.PinWorktree.
 * Use `create(PinWorktreeSchema)` to create a new message.
 */
export const PinWorktreeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 85);

/**
 * Describes the message ket.remote.v1.DeleteWorktree.
 * Use `create(DeleteWorktreeSchema)` to create a new message.
 */
export const DeleteWorktreeSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 86);

/**
 * Describes the message ket.remote.v1.WorktreeDeleted.
 * Use `create(WorktreeDeletedSchema)` to create a new message.
 */
export const WorktreeDeletedSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 87);

/**
 * Describes the message ket.remote.v1.GetSnippets.
 * Use `create(GetSnippetsSchema)` to create a new message.
 */
export const GetSnippetsSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 88);

/**
 * Describes the message ket.remote.v1.SnippetList.
 * Use `create(SnippetListSchema)` to create a new message.
 */
export const SnippetListSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 89);

/**
 * Describes the message ket.remote.v1.Snippet.
 * Use `create(SnippetSchema)` to create a new message.
 */
export const SnippetSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 90);

/**
 * Describes the message ket.remote.v1.ListDevices.
 * Use `create(ListDevicesSchema)` to create a new message.
 */
export const ListDevicesSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 91);

/**
 * Describes the message ket.remote.v1.DeviceList.
 * Use `create(DeviceListSchema)` to create a new message.
 */
export const DeviceListSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 92);

/**
 * Describes the message ket.remote.v1.Device.
 * Use `create(DeviceSchema)` to create a new message.
 */
export const DeviceSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 93);

/**
 * Describes the message ket.remote.v1.RevokeDevice.
 * Use `create(RevokeDeviceSchema)` to create a new message.
 */
export const RevokeDeviceSchema = /*@__PURE__*/
  messageDesc(file_ket_remote_v1_remote, 94);

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
 * Describes the enum ket.remote.v1.BacklogPriority.
 */
export const BacklogPrioritySchema = /*@__PURE__*/
  enumDesc(file_ket_remote_v1_remote, 3);

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
  enumDesc(file_ket_remote_v1_remote, 4);

/**
 * Since 1.19. How GitHub lands a pull request.
 *
 * @generated from enum ket.remote.v1.PullRequestMergeMethod
 */
export const PullRequestMergeMethod = /*@__PURE__*/
  tsEnum(PullRequestMergeMethodSchema);

