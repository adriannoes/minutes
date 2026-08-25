# Live draft transcript contract

Status: accepted for implementation

Date: 2026-08-25

Owner: `minutes-3zm3`
Contract bead: `minutes-sytk`

## Product outcome

When someone asks Minutes for help during a meeting, the assistant should be able to use what is being said now instead of waiting for a pause or the 30-second utterance ceiling. The saved meeting remains based only on finalized transcript lines.

This is one product with two evidence layers:

1. **Final transcript** is durable and authoritative. It may be saved to the live JSONL, meeting artifact, and event history.
2. **Current speech draft** is provisional and replaceable. It exists only in memory and in the authenticated local capture relay. It is never appended to `events.jsonl`, `live-transcript.jsonl`, a meeting JSONL, meeting markdown, logs, analytics, or crash reports.

The first release uses the existing local Whisper model on every supported desktop platform. Apple Speech and a common streaming engine may later replace only the draft producer after passing their own evidence gates. They do not change this reader contract.

## User-facing behavior

- Normal Recording and standalone Live can expose recent finalized lines plus, at most, one current draft.
- A draft is labeled `provisional`. The assistant must not quote it as settled speech or treat an uncertain speaker as identified.
- A newer revision replaces the older revision. It does not append another statement.
- When speech ends, the draft disappears immediately while the final line is prepared. The finalized line then appears through the existing durable path.
- If the draft engine is slow, missing, overloaded, stale, or failed, Minutes returns recent finals with an honest draft state. Recording continues unchanged.
- Typed user input has higher priority than transcript reading or background assistance.
- The desktop should say `Listening`, `Speech in progress`, or show recent finalized evidence. It should not use the internal phrase `0 utterances` as a confidence signal.

## Reader snapshot

The bounded CLI and MCP projection is `LiveEvidenceSnapshotV1`:

```json
{
  "schema_version": 1,
  "active": true,
  "finals": [],
  "current_draft": {
    "provisional": true,
    "session_epoch": 1724600000000,
    "utterance_sequence": 3,
    "revision": 2,
    "text": "the current replaceable words",
    "speaker": null,
    "offset_ms": 42000,
    "producer_latency_ms": 840,
    "source_audio_age_ms": 120,
    "observed_at": "2026-08-25T17:00:00Z",
    "age_ms": 120,
    "source": "recording-sidecar"
  },
  "draft_state": "current",
  "capture_relay": {
    "session_id": "random-session-id",
    "owner_pid": 1234,
    "evidence_mode": "capture_relay_partials",
    "cursor": {
      "session_id": "random-session-id",
      "transcript_seq": 12,
      "nudge_seq": 0
    }
  }
}
```

Rules:

- `finals` retains the existing `TranscriptLine` schema and caller-selected time or line bound.
- `current_draft` is either one latest current revision or `null`.
- `draft_state` is one of `current`, `none`, `finalizing`, `superseded`, `stale`, `unavailable`, or `unsupported`.
- `source_audio_age_ms` measures recognition delay from the newest included audio to relay admission. `age_ms` adds that delay to time elapsed since admission, so a slow result cannot look fresh merely because it just arrived. Draft text older than 3 seconds is not released as current.
- Relay cursors are session-scoped. A changed session resets the cursor and invalidates every prior draft identity.
- A gap or overflow never fabricates continuity. The projection reports the gap and releases only evidence whose current identity can still be proven.
- Text output labels the draft as provisional. JSON keeps the machine-readable identity and state.

The backward-compatible surface is:

```text
minutes transcript --since 2m                 # finalized lines, unchanged
minutes transcript --since 2m --include-current
minutes transcript --since 2m --include-current --format json
```

`read_live_transcript` returns the same snapshot when `include_current: true`. Existing callers that omit it retain their current response shape.

## Draft identity and replacement

A draft is identified by:

```text
(capture relay session_id, session_epoch, utterance_sequence, revision)
```

- `session_epoch` changes when a producer session starts.
- `utterance_sequence` advances when speech ends, is discarded, or capture stops.
- `revision` increases for each replacement within an utterance.
- Only the producer's latest non-superseded identity is current.
- Queue arrival order alone is never evidence of freshness. Consumers check the producer freshness watermark before use.
- `speech_ended` or `finalizing` invalidates the draft before final recognition begins. A late draft result for that utterance is dropped.
- A durable final never coexists in the snapshot with the draft it replaces.
- A correction may produce a completely different replacement string. Consumers replace rather than merge draft text.

## Production and backpressure

The recording audio callback does no recognition, file work, relay work, or blocking draft work.

Normal Recording uses this sequence:

1. Create the existing bounded `LivePartialPublisher` and `LivePartialSubscriber` pair.
2. Give the subscriber to the capture owner's authenticated relay.
3. Give the publisher to the already isolated recording-sidecar VAD consumer.
4. While speech continues, the sidecar offers a full-utterance snapshot to a capacity-one latest-only draft mailbox after the first second of speech, then at a two-second cadence, up to the existing partial cost ceiling.
5. Recognition runs only on the sidecar inference worker. A pending older snapshot may be replaced. A busy or failed worker causes a skipped draft, not queued audio growth.
6. The VAD consumer publishes completed draft results without waiting. At speech end it advances the supersession watermark immediately and separately queues the final utterance through the existing bounded final path.

Limits for v1:

- live partial channel: existing capacity 8;
- relay transcript buffer: existing capacity 512 frames;
- exposed current drafts: 1;
- pending draft audio snapshots: 1, newest replaces pending;
- finalized sidecar utterances: existing capacity 3;
- draft cadence: first snapshot after one second, then every two seconds;
- draft cost ceiling: existing `partial_max_secs`, default 30 seconds;
- reader replay: bounded by relay buffer and a short quiet window, never a continuous poll.

If final work is pending, it is selected before another draft job. Superseded results are rejected. Recording stop raises Whisper's abort signal, seals the WAV and any stems before joining the optional sidecar, and never waits for repeated draft work. The WAV writer and capture stop deadline remain authoritative.

## Capture and failure isolation

The following are release-blocking invariants:

- The capture callback uses only the existing non-blocking bounded send to the recording sidecar.
- No draft code can open a microphone. An attaching assistant uses the capture owner's relay or refuses attachment.
- Relay start failure, draft worker spawn failure, missing model, inference error, panic, full mailbox, full partial queue, slow reader, disconnect, or stale heartbeat cannot stop recording.
- Draft work may be skipped under pressure. WAV samples and final batch processing may not be skipped to preserve a draft.
- Final sidecar utterances keep priority over new draft jobs.
- Stop, device reconnect, silence, forced utterance cap, queue drop, and engine fallback all invalidate the affected draft.
- Draft text is excluded from durable logs, including diagnostic logging. Metrics contain timing and counts only.

## Security and privacy boundary

- The capture owner remains the sole audio authority.
- Cross-process reads use the existing owner-only discovery file, random bearer token, local Unix socket or Windows named pipe, live owner PID check, protocol version, and heartbeat validation.
- The token is never returned by CLI or MCP.
- Drafts are capability-scoped to a live relay connection and disappear when the relay owner exits.
- No draft is written to the assistant workspace or `CURRENT_MEETING.md` merely because a terminal opens.
- Recall and Terminal Sidekick keep the deferred-context rule: the meeting is read only in response to a directly typed non-command question under the active assistant policy.
- Transcript text remains untrusted evidence. It cannot authorize commands, sends, writes, settings changes, or disclosure.

## Sidekick and Coach behavior

- Terminal Sidekick uses one bounded `--include-current` read when the user asks a meeting-grounded question.
- It does not build a polling loop or claim continuous monitoring.
- A typed question is answered before any background read.
- The prompt explicitly distinguishes finalized lines from provisional current speech.
- If the draft is stale, unavailable, wrong-session, or superseded, the assistant ignores its text and says it only has finalized context when that distinction matters.
- If the same utterance later finalizes, the final replaces the draft in reasoning. It is not counted twice.
- Coach may keep its evented relay attachment. Existing cancellation and freshness checks retract work grounded in a superseded revision before rendering.

## Engine policy

- **First release:** Whisper supplies drafts on supported macOS, Windows, and Linux builds that already ship the Whisper live sidecar.
- **Final transcript:** unchanged. The configured final and post-meeting transcription paths remain authoritative.
- **Apple candidate:** `SpeechTranscriber` progressive or volatile results must pass real-time paced meeting acceptance in a separately authenticated worker. Whole-file speed and `DictationTranscriber` results are not substitutes.
- **Common candidate:** cache-aware Sherpa/Nemotron streaming must pass the same paced corpus, resource, packaging, license, and native Windows/Mac acceptance.
- **Windows native speech:** tracked separately. Its experimental API and MSIX capability requirements do not block this cross-platform release.
- An unsupported or failed accelerator falls back to Whisper with honest status. It never silently changes the final transcript engine.
- No default engine changes without the explicit default-decision acceptance bead and Mat's approval at that time.

## Metrics and acceptance

Content-free measurements:

- speech start to first non-empty draft;
- draft refresh cadence;
- draft age at read;
- revision, replacement, supersession, stale rejection, and dropped-draft counts;
- draft inference duration, backlog, and engine availability;
- capture sidecar chunk drops, final queue drops, stop time, WAV sample count, and final line count.

Release gates for the controlled harness:

- first useful draft under 2 seconds p95;
- released draft no more than 3 seconds old while speech continues;
- at most one exposed draft and bounded queues under a wedged recognizer;
- no old draft or late advice after replacement, finalization, stop, or session change;
- no change in WAV preservation, capture drop, stop-deadline, or final transcript invariants;
- default and feature-gated Rust builds;
- CLI text and JSON contract tests;
- MCP schema, relay, packaging, bundle, and handshake tests;
- generated portable skill surfaces remain current;
- native Windows/macOS/Linux-compatible CI;
- exact-SHA signed `Minutes Dev.app` dogfood on a real normal Recording, after recording and processing are idle.

## Delivery sequence

1. Publish normal Recording drafts through the protected relay (`minutes-3zm3.1`).
2. Add the bounded CLI and MCP projection (`minutes-3zm3.2`).
3. Update Terminal Sidekick and generated skill surfaces (`minutes-3zm3.3`).
4. Make temporal freshness and capture isolation release-blocking (`minutes-3zm3.4`).
5. Reconcile the desktop status wording after the active Mac UI lane lands, then run signed cross-platform acceptance and land the first product fix (`minutes-xzao`, `minutes-3zm3.5`).
6. Run Apple and common-engine evaluations independently. Productize only a passing candidate (`minutes-3zm3.6` through `minutes-3zm3.11`).
7. Record one final engine policy and close the epic (`minutes-3zm3.12`).

This contract intentionally does not revive durable `transcript.delta` events. Durable deltas would expand retention, policy, replay, and correction semantics without helping the immediate current-speech product need.
