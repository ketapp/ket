# Level packs

The baseline instrument and one provider-aware standard pack.

A pack is the token-reduction table ket draws on its sidebar rings, authored once and
served over HTTPS from a repository the people shipping it control. See
`crates/ket-core/src/pack.rs` for the loader.

## `decost-baseline.json` — the instrument

The five levels ket ships built in, word for word, and **nothing else**. No effort, no
model, no subagent settings.

It changes only the sentence handed to the agent at session start.

ket does not measure what a level saves by comparing sessions. That was tried three
times (cost, then tokens, then estimated dollars per prompt, each against the median
no-reduction session), and each time the figure followed task size instead of the
level: a session's tokens are ~98% cache reads, which scale with context and tool
calls. Instead, every session is priced from its own tokens, and its saving is the
output its level is assumed to trim — Light 10%, Moderate 20%, Heavy 35%, Max 50%
(`ASSUMED_OUTPUT_REDUCTION` in `crates/ket-core/src/usage.rs`). Those shares are
estimates, not measurements.

## `decost-standard.json` — the standard pack

This is the canonical schema-v2 pack. Each level has shared wording plus
explicit `claude`, `codex`, and optional `opencode` and `grok` policy blocks. ket applies
only the block matching the worktree's agent, so Claude-only environment
variables never leak into OpenCode and Codex never receives Claude flags.

The same wording, plus the settings whose effects are documented rather than assumed:

| Setting | Why it is here |
|---|---|
| `subagentModel: haiku` | Anthropic's own costs guide names the cheapest model for what subagents actually do — running tests, fetching docs, grepping logs |
| `subagentCacheTtl: 1h` | Subagents get five minutes even on a subscription, so this is the one place the TTL lever still has somewhere to go |
| `agentTeams: false` | A team is several separate Claude Code processes, each with its own context and cache; Anthropic's own measurement is roughly 7× the tokens. A cost pack grants this nowhere |
| `crossSessionInbound` | A message from another session arrives on somebody else's schedule and lands in this one's context. `hold` shows a notice, `refuse` declines |
| `effort` | Moves cost per prompt further than any narration change does — which is exactly why it is only at the two deepest levels, and why this pack cannot measure its own levels |
| `autocompact: auto` | Compacting at a natural point reads a warm prefix; compacting after a long idle reads the whole history cold |

**Accept the tradeoff knowingly.** Because levels 1 and 0 set `effort`, they land in
different comparison buckets from the levels above them. This pack saves more and proves
less. That is the right way round for a team already convinced, and the wrong way round
for one that is not — which is what the baseline pack is for.

## `decost-codex.json` — legacy schema-v1 example

Kept to exercise compatibility with schema v1. New deployments should use
`decost-standard.json`; it contains the Codex policy in each level's `codex`
block and the Claude policy beside it.

Codex's own levers turn out to be **better than Claude's in the way that matters**, because
they are enforced rather than requested:

| Setting | Values | What it does |
|---|---|---|
| `verbosity` | `low` `medium` `high` | Caps how much the model writes. Every level's `instruction` only *asks* for less; this one is the API refusing to send it |
| `reasoningSummary` | `auto` `concise` `detailed` `none` | Reasoning summaries are output tokens like any other. `none` stops paying for them |

Both are Codex-only and a Claude launch ignores them, so they are safe to put in any pack —
they simply do nothing where there is nothing to do.

`autocompact` here is a **number**, not `auto`. A token count is the one spelling both
agents accept, so this pack's compaction works on Claude too; `auto` works only on Claude
and vanishes on Codex without a word.

Values were confirmed by making Codex's own parser reject a bad one — it answers "unknown
variant `x`, expected one of `low`, `medium`, `high`" — and the deepest level's whole flag
set was run against `codex` to check it is accepted.

## What is deliberately not in any of them

- **`model`. A standing rule, not an oversight.** Pinning the main model is the largest
  single lever and it stays out. It changes what the agent can *do*, not merely how much it
  says — and, structurally, model is part of the comparison key, so a level that pins one
  can never be compared with the level above it. If a team wants its worktrees on
  Sonnet that is a conversation to have out loud, not a line a pack slips in. Do not add it
  later thinking it was forgotten.
- **`service_tier`** (Codex). Potentially the largest discount available — `flex` is the
  cheaper, latency-tolerant tier — and deliberately absent from the first pass. Codex does
  not validate it: it accepted `service_tier=zzinvalid` without a murmur, so the value goes
  straight to the API and a typo silently changes somebody's billing tier. It needs
  ket-side validation before a pack is allowed to set it, unlike `verbosity` and
  `reasoningSummary`, which Codex checks itself and refuses loudly.
- **`cacheTtl`.** ket already grants `1h` on metered accounts from its own billing
  detection and knows a subscription has the hour already. An explicit value here
  overrides that judgement and can only make it worse.

## Publishing

Serve the file from a URL that carries its own version — a GitHub raw URL bakes the commit
into the path — and configure it once, at setup:

```toml
[pack]
url = "https://raw.githubusercontent.com/<org>/<repo>/<commit-sha>/packs/decost-standard.json"
```

Pinning to a commit rather than a branch is what makes a bad publish "one machine is a
version behind" instead of "everyone updated at once".

Bump `packVersion` on every publish. Note what it costs: comparisons never cross a pack
version, so bumping resets the measured baselines underneath it. That is the honest price
of changing what the levels say, and a good reason not to do it casually.

A newly fetched pack is **staged**, not adopted — it waits beside the active one until
somebody presses the button in ket, so publishing and deploying stay separate acts. The
first pack a machine ever fetches is the exception, since there is nothing for it to differ
from.
