# Plan Zeca: the road to 0.2 and after

Internal board (2026-10-10), checked against the code at `8b59b7d` (#194). It is the one roadmap:
it joins the Zeca plan, a review of it and what was left of `road-to-0.2.md`. Eight waves; **0.2.0
ships at the end of wave 4**; mobile runs in parallel once the foundation is in.

How to read it:

- **Decisions** settles every conflict between the old plans, the review and the code. It wins
  over the documents before it.
- **Zeca's two paths** shows what Zeca gains through the harness (an API key or a local model) and
  through the CLIs (Claude Code and Codex, on the user's subscription), wave by wave.
- **Waves 0 to 7** have one step per PR, with its origin, what it needs and when it is done.
- **Start now** is week 1 as a checklist.

`road-to-0.2.md` keeps what is done, the design decisions **D1 to D8**, the invariants (its
section 2) and its notes; this file cites them by those names. Step ids here never reuse a `D`.

Three findings changed the first draft: Zeca's own work never shows as a session, and a native
mode would break that unless the event is marked (Z1); the requests Zeca would make on his own
(web, sleep, routines, the relay) run into rule 4 (G9); and the reference project is never named
outside `NOTICE` (`check-brand.sh`). All three are resolved below.

## Decisions

Each line reconciles the first draft, the review and today's code; *Decision* is the one that holds.

| Topic | Before | Decision | Why |
|---|---|---|---|
| Storage | `zeca.sqlite`, `journal/*.jsonl` and a separate audit log | A `store` crate (layer Core): one `vults.sqlite` with bundled `rusqlite` (FTS5) and `sqlite-vec`; tables `turns`, `audit`, `journal`, `traces`, `offers`, `recall`, `tasks`. `history.jsonl` and `days.json` (`app/src/history.rs`) migrate | Three stores for one thing; P1 and P8 already asked for SQLite |
| Offer ledger | In the `zeca` crate | In `core` (pure), persisted by `store` | With Zeca off, cards and the phone still work (D7) |
| Action classes | Two taxonomies | `core::policy::Class` = Read, ReadExternal, Write { destructive }, Spend, External, for agents and for Zeca | One policy in 0.2.0 |
| Expiry | A fixed 2 min | `min(120 s, limits::SERVER_DECISION_TIMEOUT)`: the server lets the hook go at 108 s, before the hook's own 110 s budget | Approving after 108 s would decide for a dead hook |
| Layers | A `check-deps.sh` (does not exist); Zeca only in Experience | Zeca's brain in a `zeca` crate (layer Connect); his body stays in Experience. `check-layers.sh` reads `cargo metadata` | Its awk reads only `[dependencies]` and misses `[target.'cfg(...)'.dependencies]` |
| Agent drivers | `AgentDriver` in `zeca` | Trait in `agents` (layer Core, no tokio); implemented in `chat`; `app` injects it | `zeca` does not depend on `chat` to dispatch work |
| Zeca's own work | Showed as Zeca's bird | Never a session. Today no hook fires for the chat (`claude --setting-sources ""`, `codex -c features.hooks=false`), and `fee18ec` keeps Zeca off the session birds. Native mode (Z1) loads the user's settings, so its hooks fire: the process carries `VULTS_ZECA=1`, the hook marks the event and `core` routes it to the chat's state (taint and activity) | Keeps today's behavior once hooks run |
| Names in code | `<lembrancas>`, Ninho | English: `<memories>`, `<today>`, `<flock>`, `/remember`, `/forget`, `/task`, `/delegate`, `/continue`; tasks are the Nest's Tasks tab; labels through i18n | `check-english.sh`; the Nest is already 0.2.0's name |
| Surfaces | An expanded panel + Settings → Zeca → Memory and Traces | One window: the **Nest**, built when opened (D6), with tabs Chat, Memory, Traces, Tasks, Roosts, Audit, History | The same screens are not built twice |
| Denying a destructive command | Zeca would deny on his own | Only a policy the user wrote may allow or deny (ADR 0014); Zeca never answers a permission | Rule 2 |
| Requests nobody asked for | Web, sleep, routines, relay with no rule | ADR 0019: each one off by default, turned on in Settings, with a budget and a local trace; never telemetry. One network gate, owned by `app` (the only crate that sees `zeca`, `link` and the update check), with a switch per feature | Rule 4 allows only the update check |
| A phone approving | A biometric signature | ADR 0017: a signature with `BIOMETRIC_STRONG` counts as a human click; D1 becomes "the island and the phone" | Rule 2 and D1 |
| Work journal | Always written | Opt-in when Zeca's memory is turned on; secrets redacted before writing | P1's note: keeping `note` is opt-in |
| Skills, `AGENTS.md`, `CLAUDE.md` | Written directly | Rule 3: backup, diff and click. Zeca's MCP goes through `--mcp-config` on the process, no config touched | Rule 3 |
| Reference project | Named | "The reference" in the repo; the name only in `NOTICE` | `check-brand.sh` |
| Release | Only `SHA256SUMS` | `actions/attest-build-provenance` in the `publish` job | Provenance before ADR 0016's signatures |
| Scope of 0.2.0 | Everything planned by 0.2.0 (ADR 0015) | 0.2.0 at the end of wave 4; waves 5 to 7 ship as 0.2.x (ADR 0020, superseding 0015) | Waves 5 to 7 and the leftovers in X come after 0.2.0 |
| Old roadmap | Waves 8, 9 and section 9 of road-to-0.2 | Absorbed here: P1→S1, P8→S2, P2→M3, P3→W1, P7→W9, section 9→W2/W4/W9/W11, C9/P9/C11→K8, C3→K9, P4→X1, P5→X2, P10→X3, P11→X4, C10→X5; P6 done (Activity A6, #188) | One board |

## Architecture

Zeca's brain joins the Connect layer, beside `chat`; the database and the safety contracts go down
to the Core layer, where they work with Zeca off.

```
Experience   app · platform · ui (island, Nest, Zeca's body)
                │ injects AgentDriver impls, owns the network gate (ADR 0019)
Connect      zeca (new: brain, memory repo, FlockReader) · link (new, phone) · chat (engines,
             AgentDriver impls) · connectors · voice · media · relay (new: a blind
             store-and-forward server, its own binary and image; depends only on link, protocol)
Core         core (+ policy, ledger) · store (new: vults.sqlite) · agents (+ AgentDriver trait)
             protocol · ipc · peer · hook · agent-config
Base         brand · secrets
```

`chat` implements the engines and the `AgentDriver` trait; `zeca` uses both, and no crate below it
knows it exists. Zeca's body (sprites, island, Nest) stays in Experience.

## Zeca's two paths

One brain (the `zeca` crate); only how memory, context and tools reach the model changes. The user
switches paths and keeps talking to the same Zeca.

| Capability | Path A: harness (API key or local) | Path B: CLI (Claude Code and Codex) | Wave |
|---|---|---|---|
| Who runs the loop | Vults (`chat`, harness) | The user's CLI | |
| Cost | The key's tokens; none on a local model | The subscription's quota | |
| The user's setup | | Native mode: the user's `CLAUDE.md`, skills and MCPs | Z1 |
| Model and effort | A picker per provider | `--model`; effort on the Codex thread | Z3 |
| Frozen context | A tiered system prompt with a cache breakpoint | `--append-system-prompt` (Claude) and `developerInstructions` (Codex), byte-identical through the conversation | M2 |
| Volatile context | `<flock>` and `<today>` at the start of the turn | The same | M2 |
| Recall | `<memories>` prefetch + a `recall` tool | Prefetch + `recall` through the `vults-zeca` MCP | M5, M7 |
| Writing memory | A `memory_propose` tool in the memory tool's format | `/remember`, extraction at the end of the conversation, `memory_propose` through the MCP | M6, M7 |
| Zeca's tools (flock, tasks) | Native in the loop | The `vults-zeca` MCP through `--mcp-config` | Z6, M7 |
| Code (bash, edits) | Not in v1: it becomes a task for an agent | The CLI's own, through the chat's cards | W3, W4 |
| Web | `web_search` and `web_fetch` through a Q-LLM; server tools on the Anthropic provider | The CLI's own WebSearch and WebFetch | Z7 |
| Taint | Per value | Per conversation, through the marked hook event | Z8 |
| Compaction | Context editing (Anthropic), native compaction (Responses), our own summary elsewhere | The CLI's | Z6 |
| When to pick it | No subscription, a local model, cheap routines and sleep | Already paying for Claude or ChatGPT | |

Product rule: on path B, Zeca is never worse than the CLI in a terminal; on path A he never runs
code himself, he delegates.

## Waves

The main line takes 16 to 19 weeks to the end of wave 5; a wave starts only when the gate before
it is green. Estimates are for one person with coding agents.

| Wave | Weeks | Gate | Release |
|---|---|---|---|
| 0 Hygiene and decisions | 1 | ADRs accepted, guards green | 0.1.x |
| 1 Foundation | 2 | Activity unchanged on the store; ledger tests | 0.1.x |
| 2 Zeca useful now | 2–3 | Native mode answers like the terminal | 0.1.x |
| 3 Memory | 3–4 | hit@3 ≥ 0.8, half the proposals accepted | 0.1.x |
| 4 Work and Operations | 4–5 | R3 smoke with the Nest, tasks and review | **0.2.0** |
| 5 Companion | 4 | | 0.2.x |
| 6 Mobile (parallel to 3–5) | 4 | Needs S3 and ADR 0017 | 0.2.x |
| 7 Closing | when there is room | none | 0.2.x |

Each step becomes a 0.1.x release when the R3 smoke passes, as `road-to-0.2.md` (*Releases*) says;
0.2.0 is tagged at wave 4's gate, and waves 5 to 7 ship as 0.2.x (ADR 0020). Wave 6's L1 to L3 start
once S3, G7 and G9 are done, if there is capacity; L4 waits for wave 4.

## Wave 0: hygiene and decisions

One week, nothing visible: the missing guards and the ADRs without which coding agents will refuse
the work (`CLAUDE.md` tells them to follow the rules). The guards (G1 to G5) touch different files
and run in parallel; the ADRs G6 to G10 all edit `CLAUDE.md`, so they merge one after another.

| # | Step | Origin | Done when |
|---|---|---|---|
| G1 | **Done.** `check-layers.sh` reads `cargo metadata` (catches `[target.'cfg(...)'.dependencies]`) and knows `store` (Core), `zeca`, `link` and `relay` (Connect; the relay depends only on `link` and `protocol`); dev-dependencies still do not count; `docs/architecture.md` lists each crate in its own step | Review | A target dependency that climbs a layer fails the script |
| G2 | **Done.** A panic hook at the start of `vults-hook`'s `main`: `std::panic::set_hook(Box::new(\|_\| std::process::exit(0)))` | Review | Test: a forced panic exits 0 with empty stdout (rule 1) |
| G3 | **Dropped (2026-10-10).** PRs keep landing as merge commits, never squashed: the history stays as it is | Review | |
| G4 | **Done.** The hook's cold start in CI with `hyperfine`, with a ceiling measured today | Review | CI fails above the ceiling |
| G5 | **Done.** `actions/attest-build-provenance` in the `publish` job | Review | The next tag ships with an attestation `gh attestation verify` accepts |
| G6 | **Done.** Accept ADR 0014: rule 2 and the test that pins it change in the same PR | road-to-0.2 section 9 | ADR accepted; policies stay off in code until S2 and W2 |
| G7 | **Done.** ADR 0017: a decision signed on the phone with `BIOMETRIC_STRONG` counts as a click; D1 becomes "the island and the phone"; `CLAUDE.md`'s *Stack* allows Kotlin for the Android shell only, and *Priorities* names Android as the one exception to "Linux only" | Plan + review | ADR accepted; D1 updated in road-to-0.2 |
| G8 | **Done.** ADR 0018: Zeca as an agent. Brain in the Connect layer, typed actions with a class, never answers a permission, his work is never a session | Plan + ADR 0010 | ADR accepted; `CLAUDE.md` (*Architecture*, layers) and `docs/architecture.md` updated |
| G9 | **Done.** ADR 0019: requests on the user's behalf (web, sleep, routines, relay, push) off by default, each with its own switch in Settings, a budget and a trace; never telemetry | New | ADR accepted; rule 4 in `CLAUDE.md` cites 0019 |
| G10 | **Done.** ADR 0020, superseding 0015: 0.2.0 is wave 4's gate; waves 5 to 7 ship as 0.2.x; 1.0 still means "done for Linux" | New | ADR accepted; road-to-0.2 *Releases* points at it |
| G11 | **Done.** This plan in the repo, in English, with no reference name; road-to-0.2 points at it | New | Done with this file: `check-english`, `check-brand` and `docs.yml` green |

Each ADR ships with its translation in `docs/pt-br/adr/` and its source mark, as `CLAUDE.md` asks.
`plan-more-agents.md`'s A0 takes the next free ADR number after these.

## Wave 1: foundation

Two weeks. Everything after goes through here: one database, the ledger and the policy in `core`,
traces, evals and the agent trait. No screen changes.

| # | Step | Origin | Needs | Done when |
|---|---|---|---|---|
| S1 | **Done.** `store` crate (Core): `vults.sqlite`, versioned migrations, bundled `rusqlite` with FTS5; migrates `history.jsonl` and `days.json` | P1 + review | G1 | Activity reads the same before and after; `cargo deny check` green |
| S2 | **Done.** Append-only audit: a click on a card, an *Always* rule, an agent config written, an action by Zeca; who (human, rule, policy, system), what, on what | P8 | S1 | Every answer to a card writes a row; a test that the table refuses update and delete |
| S3 | **Done.** `core::policy` (classes) and `core::ledger` (`Offer`, `Binding`, `Device`, `Refused`; binding = SHA-256 of the whole request; expiry = `min(120 s, limits::SERVER_DECISION_TIMEOUT)`; single use; a counter per device). The island's clicks go through the ledger | Plan + review | G6 | Tests for replay, expired, a binding that differs, double use; behavior unchanged |
| S4 | **Done.** `proptest` on `reduce`: random `Input` sequences with time; the invariants of road-to-0.2 section 2 and rule 2 for every `Intent` | Review | S3 | 256 cases in CI; a minimal failing case reproduces |
| S5 | `cargo-fuzz` on `protocol`'s decode and `agents`' event parsing | Review | | Targets in the repo, a corpus from the fixtures, a weekly job |
| S6 | Traces: a span per model call, tool and action, named after the OpenTelemetry GenAI conventions, written to `store` | Plan | S1 | A chat turn shows its tokens, cache and cost |
| S7 | An eval harness with cassettes (`--features evals`): 20 first cases on context assembly, policy and injection | Plan | S3 | Green in CI; a prompt change without an eval does not pass review |
| S8 | An `AgentDriver` trait in `agents` (start, send, interrupt, events; `async fn` without tokio). Claude stream-json and the Codex app-server implemented in `chat`; `app` injects them | Plan + review | G1 | `chat`'s tests pass unchanged |
| S9 | `zeca` crate (Connect): a `FlockReader` with no path to an `Intent`; a memory repository through `gix` | Plan | G1, G8 | A compile-fail test; init is idempotent; each write is a commit |
| S10 | The network gate in `app` (ADR 0019): a switch, a daily budget and a local trace per feature; the connectors' poll (GitHub through `gh`) moves behind it first | G9 | G9, S1 | A test with every feature off counts zero requests from the gate; the GitHub card works as before when connected |

With Zeca off, `zeca` is inert: no process, no new file, no request.

## Wave 2: Zeca useful now

Two to three weeks. At the end, in a real repo, using Zeca is no longer worse than opening a
terminal. Z1, Z2 and Z3 do not need the foundation and can start with wave 1.

| # | Step | Path | Needs | Done when |
|---|---|---|---|---|
| Z1 | **Native mode** (on by default, with a badge): Claude with `--setting-sources user,project` and the user's MCPs; the process carries `VULTS_ZECA=1`; the hook marks the event (protocol 6) and `core` routes it to the chat's state | B | | The same answer as `claude` in a terminal; no new session in the flock |
| Z2 | **"Always in this conversation"** on the chat's card, bound to the hash of the whole command, gone with the conversation | B | | A different command with the same displayed text does not pass |
| Z3 | **Model and effort** per engine, remembered per roost | A, B | | A picker at the top of the chat |
| Z4 | **Many conversations** in `store`: list, resume (resume id or thread), rename, delete; switching engines opens a new conversation and keeps the old one | A, B | S1 | Reopen the app and go on with yesterday's conversation |
| Z5 | **The Nest** (a window built when opened, D6) with its Chat tab: full markdown; the persona in `persona.md`, in English, answering in the user's language | A, B | Z4 | The new webview's memory measured and written down, as in E9; the bubble stays for short answers |
| Z6 | **A tool loop in the harness**: Anthropic with context editing and caching; OpenAI through the Responses API with native compaction; chat completions only for OpenRouter and Ollama; up to 8 iterations; Stop midway | A | S3, S6, S7 | A mock with chained `tool_use`; history never passes 50 % of the window |
| Z7 | **Web**: `web_search` (Brave, Tavily or SearXNG) and `web_fetch`, always through a Q-LLM; server tools on the Anthropic provider | A | G9, Z6 | Eval: a page with a malicious instruction produces no `Action` |
| Z8 | **Taint**: per value in the harness; per conversation on path B, through Z1's marked event | A, B | Z1, S3 | The next card warns that the conversation read outside content |
| Z9 | **Flock tools** in the harness: `flock_list`, `session_detail`, `pending_asks`, `usage`, `github_snapshot` | A | S9, Z6 | "What is Codex doing?" is answered right without pasting anything |

## Wave 3: memory

Three to four weeks. Memory works the same on both paths, is only written by an accepted proposal
and becomes a commit in the memory repository.

| # | Step | Origin | Needs | Done when |
|---|---|---|---|---|
| M1 | Memory store: `system/USER.md` (1,375 chars), `system/MEMORY.md` (2,200), `system/persona.md`, `roosts/<repo>.md` (2,000), `notes/` read on demand. Markdown with frontmatter; add, replace and remove by a unique substring; an error when full; an injection scan; a replaced fact gains `until:` | Plan + MemFS | S9 | Tests for limits, an ambiguous substring, injection and time |
| M2 | `ContextBuilder`: the frozen part in the system prompt, byte-identical through the conversation; the volatile part in the turn (`<flock>`, `<today>`, `<memories>`), on all three engines | Plan | M1, Z4 | Snapshot: the system prompt's hash does not change between turns |
| M3 | Roosts: sessions grouped by repo, with branch, PR and CI from the GitHub snapshot; the key of project memory | P2 | S1 | A Roosts tab in the Nest; the roost's memory joins when a conversation starts in its repo |
| M4 | Journal, opt-in when memory is turned on: `core`'s typed events in `store`, secrets redacted before writing, a daily summary by a cheap model | Plan + P1's note | S1, G9 | A real day summed up in under 2 KB; test tokens never written |
| M5 | Hybrid recall: FTS5 + `sqlite-vec` + a local embedding through Ollama, merged by reciprocal rank fusion; a prefetch of up to 3 passages | Plan | S1, M4 | Eval: hit@3 ≥ 0.8 on 30 questions about earlier days |
| M6 | Writing: `/remember`, `/forget`, extraction at the end of a conversation, *Pending* in the Nest's Memory tab; accepting is a commit, undoing a revert | Plan | M1, Z5 | ≥ 50 % of proposals accepted in real use; no duplicates |
| M7 | The `vults-zeca` MCP with `rmcp`, for Zeca through a CLI and for the agents he dispatches: `recall`, `memory_propose`, `roost_memory`, `flock_*`, `journal_query`, `ask_user` | Plan | M1, M5 | Zeca through Claude saves a memory by tool, with a card |
| M8 | Zeca's sleep: idle or nightly consolidation that dedupes, rewrites and prepares the debrief; it only reads summaries already quarantined | Plan | M4, M6, G9 | No duplicates after 7 days of use |
| M9 | Shared project memory: propose `@AGENTS.md` in each repo's `CLAUDE.md`, with backup, diff and click | Plan | M3 | Claude and Codex read the same project memory |

Gate: recall at hit@3 ≥ 0.8 and half the proposals accepted before wave 4 dispatches agents with
memory.

## Wave 4: work and Operations (0.2.0)

Four to five weeks, and this wave is 0.2.0. Zeca creates tasks, dispatches them to the right agent
in an isolated worktree, reviews the result and learns which agent works best in each roost.

| # | Step | Origin | Needs | Done when |
|---|---|---|---|---|
| W1 | Capabilities per agent: what it asks, approves, shows of a diff, how it stops (C7's notes) and which sandbox it has; a *doctor* view | P3 | | A table per agent in the Nest; feeds the router and the policy |
| W2 | Policy engine (classes from `core::policy::classify` on the card's full target, never the cut one; a shell command with metacharacters or an interpreter is never allowed by a pattern alone): written by the user, seen as a diff, accepted by a click; answers allow, deny or ask first; what an agent can *see* is apart from whether *this call, now* passes; a deny beats an allow; destructive always asks; everything in the audit | Section 9 + ADR 0014 | G6, S2, S3, W1 | Test: no policy answers without the consent record |
| W3 | The Nest's Tasks tab and `/task`: goal, acceptance criteria, roost, owner, status and cost; GitHub Issues when connected | Plan + section 9 | M3, Z5 | A task with criteria created in one message |
| W4 | Starting sessions from here: a worktree per task, the native sandbox (bubblewrap for Claude Code, Codex's sandbox), hooks on (the project's breed), the `vults-zeca` MCP through `--mcp-config`; pausing only the sessions we started: ask it to stop, wait, then end it (`continue: false`, `turn/interrupt`), never a watched one (D8). Section 9 said "in the user's terminal": a worktree with a sandbox replaces it, and the session still shows in the flock | Plan + section 9 + C7 | S8, W3, M7 | Two tasks in parallel without conflict, both in the flock |
| W5 | Context pack: one template (Objective, Constraints, Done, In progress, Decisions, Files, Failures, Next) for handoff, compaction, summary and delegation | Plan | M2 | Eval: a Claude → Codex handoff finishes the task without explaining it again |
| W6 | Review loop: tests, diff and criteria, up to 3 rounds; the cross review uses Codex's `review/start` when Claude wrote it, and Claude when Codex did | Plan | W4, W5 | Eval: ≥ 8 of 10 planted bugs sent back |
| W7 | Handoff to a watched session (`/continue`): folder and id from the hooks, only when the session is stopped with no live process, one at a time, expires in 10 min | Plan + the reference | W4 | An instruction to a session alive in a terminal is refused |
| W8 | Router and `/delegate`: starting rules per roost and quota (above 85 % goes to the other agent); the score per agent, roost and kind replaces the rules after 10 tasks | Plan | W1, W6 | The card shows the choice and why |
| W9 | Cost and budgets: tokens and cost from the agent's stop and from the OTLP telemetry Claude Code and Codex export (received on loopback only), marked subscription or API; a warning at 80 %; a "hard stop" only means no new session is started from here, plus a notification; it never stops or blocks an agent we only watch | P7 + section 9 | S6, W4 | Cost per task and per roost in the Nest |
| W10 | An ACP driver for Gemini CLI, OpenCode, Copilot CLI and the rest of the registry | Plan | S8 | Gemini CLI takes a task from the Tasks tab |
| W11 | The whole Nest: History, Roosts, Tasks, Memory, Traces, Audit, Usage | Section 9 | W3, W9, S2 | The R3 smoke passes with the Nest |

Gate: the R3 smoke with the Nest, tasks and review; then the 0.2.0 tag.

## Wave 5: companion

Four weeks, in 0.2.x releases. Zeca learns when to speak, tells what happened, runs routines and
learns from the flock. *When* is always a rule in `core`; the model only writes *what* to say.

| # | Step | Origin | Needs | Done when |
|---|---|---|---|---|
| K1 | Attention engine for Zeca's lines: now, debrief and never; up to 4 unasked lines per hour; silence in full screen or focus; presence from the screen lock (as C5). Never delays or hides a card | Plan + C4 | S3 | A `core` test: with the budget spent, a card still opens the island (ADR 0009, D5) |
| K2 | Debrief on return and the day's opening: C5's digest + tasks + memory, written by Zeca | Plan + C5 | K1, M4, W3 | The debrief is opened on most returns |
| K3 | Card explanation: one line per permission, a risk and out-of-scope warning; approve obvious reads in a batch, always with a click | Plan | Z6, W1 | Average time a card waits goes down |
| K4 | Routines: morning brief, nightly CI, dependencies, watching a page | Plan | G9, W3 | A routine created in conversation, editable in the Nest |
| K5 | Learning from the flock: a command denied 3 times, a correction repeated in prompts to agents, the same CI step failing → a proposal for the roost or a diff to `AGENTS.md` | Plan | M6, M9 | ≥ 1 useful proposal per week of use |
| K6 | Skills in the `SKILL.md` format; writing to `~/.claude/skills` follows rule 3 | Plan | M6 | A skill Zeca wrote used by Claude in a task |
| K7 | Persona and mood: a mood from the flock (calm, alert, worried, celebrating) drives animation and tone; he looks at the bird he talks about | Plan | K1 | A visual test per mood |
| K8 | Voice on the same brain: an opt-in realtime model (replaces C9), open mic (P9) and moving-around commands (C11); never answers a card | C9, P9, C11 | Z6, G9 | A spoken question uses `flock_list` |
| K9 | A command palette that also takes `/task`, `/delegate` and `/continue` | C3 | W3 | The palette never answers a card (D1) |
| K10 | Zeca as an ACP agent: Zed, JetBrains and Neovim talk to him, with memory and the flock | Plan | M7 | A Zeca session in Zed answers about the flock |

## Wave 6: mobile, Android + Linux

Four weeks, parallel to waves 3 to 5: L1 to L3 need only the ledger (S3), ADR 0017 and ADR 0019; L4
waits for wave 4 (W3, W7). The Linux desktop is the source of truth and the one that acts; Android
only decides; the relay is blind. iOS and macOS come later without changing the protocol.

| # | Step | Needs | Done when |
|---|---|---|---|
| L1 | `crates/link` (Connect): types, X25519 + XChaCha20-Poly1305, pairing by QR (public key, relay URL, a one-time secret for 2 min), on `core`'s ledger. Before it ships: the devices' counters persisted in `store` (they live in memory since S3), a counter of `u64::MAX` refused, and the card's folder snapshotted when it is offered, so an Always from the phone is scoped to the folder the user saw | S3, G7 | Tests for replay, expired, a binding that differs, a revoked device and a signature without biometrics |
| L2 | `crates/relay` (axum, store-and-forward, only encrypted envelopes with a TTL, a Docker image) + the desktop client + a QR in Settings; transport `relay` or `direct` (LAN or tailnet) | L1, G9 | The whole flow with a command-line test client |
| L3 | Android MVP in Kotlin/Compose with the Rust core through UniFFI and `cargo-ndk`: pair, see the flock, deny without biometrics, approve and answer with biometrics (a Keystore key with `BIOMETRIC_STRONG` per use), push through UnifiedPush with FCM as a fallback | L2 | An approval at 100 s works; past 108 s it is refused (`limits::SERVER_DECISION_TIMEOUT`) |
| L4 | Handoff, tasks and Zeca's proposals from the phone; the screen lock (C5, `org.freedesktop.ScreenSaver`) takes Zeca to the phone (Live Updates on Android 16+) and unlocking waits 30 s | L3, W3, W7 | A task accepted on the phone shows in the Tasks tab |
| L5 | Service actions through `connectors`, GitHub first (re-run CI, approve a PR), with the same one-time token | L3 | An action offered more than 5 min ago is refused |

Policy on the phone: deny and dismiss without biometrics; approve, answer, handoff, accept a task and
a service action with biometrics; sending a message or moving money does not exist in the protocol.

## Wave 7: closing

What is left of road-to-0.2 and what was put off on purpose; it goes in when there is room, with no
gate.

| # | Step | Origin |
|---|---|---|
| X1 | Side panel: sessions, card queue, activity | P4 |
| X2 | Spike: a bird on the desktop (a small surface moved by its margins) | P5 |
| X3 | A voice or speed per vult and per agent | P10 |
| X4 | Vercel deployments per roost, token in the keyring | P11 |
| X5 | A personal voice dictionary; cloud transcription opt-in | C10 |
| X6 | A headless browser for Zeca (`chromiumoxide`), with its own profile, never the user's browser | Plan, put off |
| X7 | Windows beta and macOS beta | `CLAUDE.md`, *Priorities* |
| X8 | Perch together: a flock's sessions side by side (only if asked) | `plan-breeds-and-flocks.md` R3 (board deleted; R1 and R2 done in #179) |
| X9 | Open terminal on more desktops: GNOME Wayland (a Shell extension), Hyprland and Sway (`hyprctl`, `swaymsg`), terminals by name (Ghostty, Alacritty tabs) | `plan-jump-beyond-kde.md` *Out of scope* (board deleted; done in #160) |

Other open boards run beside this one: `plan-more-agents.md` (Copilot CLI, Factory Droid, Cursor,
later agents).

## Guarantees: what must not break

Each invariant has an automatic guard; a step that weakens a guard does not pass review.

| Invariant | Guard today | New guard |
|---|---|---|
| Never block an agent (rule 1) | The hook's tests | Panic hook (G2), fuzzing (S5), a cold-start ceiling (G4) |
| A permission only by a human click or recorded consent (rule 2, ADRs 0014 and 0017) | `only_decide_can_respond` | `proptest` over every `Intent` (S4), the ledger (S3), the policy test (W2) |
| An offer never outlives the hook | | Expiry = `min(120 s, limits::SERVER_DECISION_TIMEOUT)` (S3, L3) |
| Zeca never answers a permission | None in code; ADR 0010 promises a handle that cannot build `Decide` or `DecideAlways` | `FlockReader` with no `Intent` (S9), ADR 0018 |
| Zeca's work never becomes a session | No hooks in the chat's processes | A marked event routed in `core` (Z1) |
| Nothing leaves the machine unless the user turned it on (rule 4, ADR 0019) | The update check is off | One network gate in `app` with a switch per feature; test: all off, zero requests (S10, then Z7, M8, K4, L2) |
| Outside content does not trigger an action | | Q-LLM, per-value taint, injection evals (Z7, Z8, S7) |
| An agent's config only with backup, diff and click (rule 3) | `agent-config` | MCP through `--mcp-config` with no write; skills and `AGENTS.md` through the same flow (M9, K6) |
| A stable prompt through the conversation | | A snapshot of the system prompt's hash (M2) |
| Secrets are never written | The keyring (rule 4) | Redaction before the journal, recall and traces (M4, S6) |
| Zeca's lines never hide or delay a card (ADR 0009, D5) | D5's test | K1's test with the budget spent |
| Layers | `check-layers.sh` (awk) | `cargo metadata` (G1) |
| Language and brand | `check-english`, `check-brand`, `check-i18n` | The same, on this plan too |

## Product metrics

All from the local traces and audit; nothing leaves the machine. They say whether Zeca beats "using
any AI".

| Metric | Target | From |
|---|---|---|
| Conversations with Zeca per day ÷ sessions opened straight in a terminal | Up week after week | Wave 2 |
| Recall: hit@3 on the eval questions | ≥ 0.8 | Wave 3 |
| Memory proposals accepted | ≥ 50 % | Wave 3 |
| Tasks finished per week without going to a terminal | Up week after week | Wave 4 |
| Planted bugs sent back by the review | ≥ 8 of 10 | Wave 4 |
| Cost per finished task, per agent | Down with the score | Wave 4 |
| Unasked lines dismissed unread | < 20 % | Wave 5 |
| Average time a card waits | Down after K3 | Wave 5 |
| Debrief opened on return | Most returns | Wave 5 |

## Start now

Week 1 is all of wave 0 plus Z1, in parallel worktrees; week 2 opens S1, S3, S8 and S9.

- [x] G1: `check-layers.sh` on `cargo metadata`
- [x] G2: panic hook in `vults-hook`
- [x] G4: `hyperfine` of the hook in CI
- [x] G5: build attestation in `publish`
- [x] G6: ADR 0014 accepted and rule 2 rewritten
- [x] G7: ADR 0017 (the phone), *Stack* with Kotlin for the Android shell
- [x] G8: ADR 0018 (Zeca as an agent) and the layers updated
- [x] G9: ADR 0019 (requests on the user's behalf) and rule 4
- [x] G10: ADR 0020 (0.2.0 at wave 4), superseding 0015
- [x] G11: this plan in the repo
- [ ] Z1: native mode (needs only protocol 6)

How to work a step, as the repo already does (`plan-fourth-review.md`, *How to work a step*): study
and write what you learn in *Notes*; mark `doing (<branch>)` here; your own worktree from `main`;
tests, docs and the three translations in the same PR; `flock-tester`, then `flock-reviewer`, at
most two rounds of fixes; PR, a review, CI green, a merge commit (never a squash); mark **Done** in
the same PR.

The base prompt for a coding agent, one session per step:

```markdown
Implement step <ID> of docs/dev/plan-zeca.md: <title>.

Before coding: read CLAUDE.md, docs/architecture.md, the step's section in the plan and the
references for its area. Write what you learned in Notes.

Done when:
- <copied from the board>

Rules:
- Code, comments, docs and commits in English; UI sentences through t() with pt-BR, es and zh.
- Only the step's crates; no refactor mixed with a feature.
- Every action with an effect goes through core::policy and the ledger; Zeca never builds Decide or DecideAlways.
- No new network request without ADR 0019's switch, through the network gate in app.
- Tests and eval cases first; run CLAUDE.md's "Before every commit" list.
- Never write the reference project's name (check-brand.sh).

At the end, list what was left out and why.
```

## Out of scope

The cut protects what sets Zeca apart: the flock, memory and safe orchestration.

| Item | Decision | Why |
|---|---|---|
| Zeca approving another agent's permission | Never | Flock content may carry an injection; it would bypass the user's approvals |
| Zeca running code in the harness | Not in v1 | Code is the agents' work; Zeca coordinates |
| A logged-in browser or the user's personal credentials | Never | Zeca has his own profile and keys |
| A third-party agent framework | No | The loop is small; owning it allows taint, the ledger and traces |
| A messaging gateway (Telegram and the like) | No | Zeca lives on the desktop, the editor and the phone |
| A duel (the same task on two agents) | Put off | Doubles the cost; the score teaches the same for less |
| A2A | Put off | ACP and MCP cover coding agents today |
| Wake word | No | Voice by shortcut or opt-in realtime (road-to-0.2 section 12) |
| Agents started without permissions | Never | Rule 2; some orchestrators' default, not ours |

## References by area

Links checked on 2026-10-10. The reference project is named only in `NOTICE`.

| Wave | Reference | What to take |
|---|---|---|
| 1, 2 | [Anthropic Engineering](https://www.anthropic.com/engineering) | Building effective agents; Effective context engineering; Effective harnesses for long-running agents; Writing effective tools for agents; Beyond permission prompts |
| 2, 3 | [Effective context engineering](https://anthropic.com/engineering/effective-context-engineering-for-ai-agents) | The smallest set of useful tokens; just-in-time context; notes outside the window |
| 2 | [Claude: manage tool context](https://platform.claude.com/docs/en/agents-and-tools/tool-use/manage-tool-context) | Tool search, programmatic tool calling, caching and context editing |
| 2, 3 | [Context editing and the memory tool](https://claude.com/blog/context-management) and its [cookbook](https://platform.claude.com/cookbook/tool-use-memory-cookbook) | Clearing tool results; the memory tool's format |
| 2 | [Hermes: architecture](https://hermes-agent.nousresearch.com/docs/developer-guide/architecture) and [compression and caching](https://hermes-agent.nousresearch.com/docs/developer-guide/context-compression-and-caching) | A tiered prompt; compaction at 50 % with a structured summary; never switch models midway |
| 3 | [Hermes: persistent memory](https://hermes-agent.nousresearch.com/docs/user-guide/features/memory) | Limits, a frozen snapshot, substring ops, staged writes, an injection scan |
| 3 | [Letta: context repositories](https://www.letta.com/blog/context-repositories), [MemFS](https://docs.letta.com/concepts/memfs) and [sleep-time compute](https://www.letta.com/blog/sleep-time-compute) | Memory in git, `system/` always in context, sleep off the critical path |
| 3 | [sqlite-vec](https://github.com/asg017/sqlite-vec) and [gitoxide](https://github.com/GitoxideLabs/gitoxide) | Vectors in the same SQLite; `gix` to version memory |
| 3 | [AGENTS.md](https://agents.md/) and [Agent Skills](https://agentskills.io) | Canonical project memory; portable skills |
| 3 | [rmcp](https://github.com/modelcontextprotocol/rust-sdk) | The `vults-zeca` MCP server |
| 4 | [Codex app-server README](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md) | `generate-json-schema` for the Rust types; `thread/*`, `turn/*`, `review/start` |
| 4 | [Claude Code: hooks](https://code.claude.com/docs/en/hooks) and [sandboxing](https://code.claude.com/docs/en/sandboxing) | Marked events; bubblewrap and a network proxy for the workers |
| 4 | [sandbox-runtime](https://github.com/anthropic-experimental/sandbox-runtime) | A sandbox for Zeca's own runs, no container |
| 4 | [Agent Client Protocol](https://agentclientprotocol.com/protocol/overview) and its [Rust SDK](https://github.com/agentclientprotocol/rust-sdk) | The ACP driver (W10); Zeca as an ACP agent (K10) |
| 4 | [Vibe Kanban](https://github.com/BloopAI/vibe-kanban) | An `Executor` trait in Rust, a worktree per task, a workflow in SQLite. Not taken: agents without permissions by default |
| 4, 6 | The reference (see `NOTICE`) | A safe handoff to a watched session; typed intents with a runner; action tokens from the phone |
| 1, 2 | [CaMeL](https://github.com/google-research/camel-prompt-injection) and [Simon Willison's analysis](https://simonw.substack.com/p/camel-offers-a-promising-new-direction) | A Q-LLM with no tools; capabilities per value; a policy before each tool |
| 1, 4 | [OTel GenAI semantic conventions](https://github.com/open-telemetry/semantic-conventions-genai) | Span names; the telemetry Claude Code and Codex already export |
| 6 | [UniFFI](https://github.com/mozilla/uniffi-rs) and [UnifiedPush](https://f-droid.org/2022/12/18/unifiedpush.html) | The Rust core in Kotlin; push without Google |

## Notes

(Add what each step learns here.)
