# Remembered voice ownership

## Decision

A remembered voice is an explicit device-level object. It is independent from ordinary dictation, anonymous speaker attribution, and conversation sessions. Creating one requires a named user action that states what derived voice data will be stored. The presence of a remembered voice never changes default dictation.

This memo defines a deferred product boundary. Plan 012 covers only dev-time similarity comparison and does not implement this store.

## Ownership

`dictate` owns profile IDs, local paths, atomic metadata updates, model compatibility, and list, inspect, rename, and delete actions. `dictate-speech` accepts selected clean speech and returns a model-bound embedding; it does not decide retention or inclusion policy. `dictate-ui` presents explicit remember and deletion choices. `dictate-dev` owns diagnostic profile comparisons.

A profile can be created from any explicitly selected, consented audio source that meets the evidence rule. A future conversation session may supply exclusive speaker segments, but the profile store does not depend on conversation storage.

## Stored data

A remembered voice stores:

- an opaque profile ID;
- a user-chosen label;
- one or more embeddings made only from exclusive, non-overlapping speech interiors;
- embedding model identity and SHA-256 digest;
- profile format version;
- creation and update times;
- optional source provenance that contains opaque local source IDs, not raw audio excerpts.

Raw enrollment audio is not copied into the profile store. A separately retained diagnostic WAV or conversation recording remains owned by its original feature and follows that feature's deletion policy.

## Local contract

The profile store is private to the local user and supports:

```text
dictate voices list --json
dictate voices inspect <profile-id> --json
dictate voices create --label <label> --wav <path>
dictate voices rename <profile-id> <label>
dictate voices delete <profile-id>
```

These command names record the ownership boundary; they are not approved implementation work. An eventual UI must provide the same create, inspect, rename, and delete actions.

Creation is atomic. If the source contains several diarized speakers, overlap, too little exclusive speech, incompatible model bytes, or invalid embeddings, no profile is written. Rename changes only profile metadata. Delete removes the vectors and local label. Deleting a source WAV or conversation does not silently delete a profile that was separately confirmed; deleting a profile does not alter session-only labels.

## Identity versus policy

A profile comparison returns similarity evidence and uncertainty. It does not decide whether transcript text is included. Any future personal-isolation mode must name selected profile IDs explicitly and must preserve the unfiltered result when identity or overlap is uncertain.

No profile may be learned from normal dev captures merely because they are likely to contain the device owner. A user must select the audio and confirm creation.

## Gates before implementation

1. Normal use demonstrates a need for persistent voice identity beyond the dev-only probe.
2. Plan 012 shows useful model-bound similarity without choosing a deletion threshold.
3. Model provenance, redistribution, release CPU, and memory gates clear.
4. Create, inspect, rename, and delete behavior receives product review.
5. The storage schema, migration behavior, and atomic failure tests are planned separately.

## Deferred behavior

- Automatic learning from ordinary dictation.
- Hidden enrollment from labels or filenames.
- Cloud synchronization.
- Default filtering of unknown speakers.
- Personal isolation and target-speaker extraction.
