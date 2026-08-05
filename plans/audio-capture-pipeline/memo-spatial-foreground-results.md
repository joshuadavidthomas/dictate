# Native stereo foreground transform results

## Final verdict

Do not ship a spatial transform from Plan 009. Channel selection, raw side extraction, integer delay-and-sum, and constrained fixed mid/side cancellation produced no winner. Controlled mono calibration failed the acoustic gate before ASR, so Plan 009 closes with a no-go verdict and leaves production averaging unchanged.

The first reading overstated user harm. Literal WER counted `one` versus `1` as five word errors and `TV` versus `T V` as two errors, even though those transcripts kept the spoken content. Raw side extraction did cause real damage: it canceled centered speech, exposed TV words in the mixed clip, and changed quiet-user words. The delay variants preserved user content, but their apparent TV gains came from changes against one ASR-generated TV transcript rather than an independent reference or measured attenuation. No declared variant cleared the shipping gate.

## Inputs

The required consented files remained under `/tmp/dictate-channel-probe/` as 10-second, 48 kHz stereo float WAVs:

| Clip | Target reference | Interferer evidence |
|---|---|---|
| `user-only.wav` | `This is Josh speaking close to the laptop. The blue notebook is beside the window. One, two, three, four, five.` | none |
| `tv-only.wav` | none | The average variant's 26-word raw transcript, used only as a stable overlap anchor for the other TV-only outputs |
| `mixed-dialogue.wav` | `This is Josh speaking over the TV. One, two, three, four.` | `Yeah, chat.`, the two opening words decoded only by the side output |

The TV-only overlap figure is not WER against a human transcript. It measures how much of the average variant's recognized TV dialogue survives in each output. The mixed marker is also recognizer evidence rather than an independent transcript, so it cannot clear a shipping gate.

A fourth consented stereo file, `/tmp/dictate-speaker-corpus/josh-near-quiet.wav`, supplied the quiet-user check. Its intended text was `This is Josh speaking for the Dictate voice test. The blue notebook is beside the window. One, two, three, four, five.`

## Commands

The headless probe produced the JSON used below:

```console
DICTATE_BUILD=dev cargo run --quiet -p dictate --features dev-tools -- spatial-probe \
  /tmp/dictate-channel-probe/user-only.wav \
  --target-reference 'This is Josh speaking close to the laptop. The blue notebook is beside the window. One, two, three, four, five.'

DICTATE_BUILD=dev cargo run --quiet -p dictate --features dev-tools -- spatial-probe \
  /tmp/dictate-channel-probe/tv-only.wav \
  --interferer-reference "Yes, sir. What about oh I know. With all due respect, I don't like a lot of sense. No, sir, he said, if three wounded."

DICTATE_BUILD=dev cargo run --quiet -p dictate --features dev-tools -- spatial-probe \
  /tmp/dictate-channel-probe/mixed-dialogue.wav \
  --target-reference 'This is Josh speaking over the TV. One, two, three, four.' \
  --interferer-reference 'Yeah, chat.'

DICTATE_BUILD=dev cargo run --quiet -p dictate --features dev-tools -- spatial-probe \
  /tmp/dictate-speaker-corpus/josh-near-quiet.wav \
  --target-reference 'This is Josh speaking for the Dictate voice test. The blue notebook is beside the window. One, two, three, four, five.'
```

The deterministic command check used a one-second 440 Hz, 48 kHz float stereo WAV. Its JSON parsed, contained all 11 variants, and reported 16 kHz output metrics for every row.

## Source metrics

The probe defines positive lag as a right-channel delay relative to the left channel. It searched and generated every integer delay within a 2 cm path difference, or ±3 samples at 48 kHz.

| Clip | Left RMS | Right RMS | Mid RMS | Side RMS | Correlation | Best lag |
|---|---:|---:|---:|---:|---:|---:|
| TV only | 0.033557 | 0.034098 | 0.033688 | 0.003074 | 0.983613 | +2 |
| User only | 0.035097 | 0.034976 | 0.034976 | 0.002050 | 0.993159 | 0 |
| Mixed dialogue | 0.035394 | 0.034839 | 0.034946 | 0.003470 | 0.980593 | 0 |
| Near quiet | 0.030206 | 0.029934 | 0.029552 | 0.005558 | 0.931707 | 0 |

The lag split matches the earlier native-channel probe: the isolated TV was off-axis, while the nearby user was centered. The valid mixed clip still measured zero lag because the nearby user dominated it.

## User-only results

The reference contains 20 normalized words. `digits` denotes the same sentence with `1, 2, 3, 4, 5` in place of the five spelled-out numbers. `joined digits` ends in `12345`.

| Variant | Delay | Raw transcript | Target edits / WER | Output RMS |
|---|---:|---|---:|---:|
| average | | Exact reference | 0 / 0.0% | 0.034047 |
| left | | digits | 5 / 25.0% | 0.034157 |
| right | | digits | 5 / 25.0% | 0.033978 |
| side | | Exact reference | 0 / 0.0% | 0.001176 |
| delay-and-sum | -3 | joined digits | 5 / 25.0% | 0.033976 |
| delay-and-sum | -2 | digits | 5 / 25.0% | 0.034004 |
| delay-and-sum | -1 | Exact reference | 0 / 0.0% | 0.034030 |
| delay-and-sum | 0 | Exact reference | 0 / 0.0% | 0.034047 |
| delay-and-sum | +1 | Exact reference | 0 / 0.0% | 0.034044 |
| delay-and-sum | +2 | digits | 5 / 25.0% | 0.034026 |
| delay-and-sum | +3 | digits | 5 / 25.0% | 0.034005 |

## TV-only results

Coverage counts ordered words shared with the average variant's 26-word TV transcript.

| Variant | Delay | Raw transcript | TV words / coverage | Output RMS |
|---|---:|---|---:|---:|
| average | | `Yes, sir. What about oh I know. With all due respect, I don't like a lot of sense. No, sir, he said, if three wounded.` | 26 / 100.0% | 0.033007 |
| left | | `Yes, sir. What? Oh well, with all due respect and don't make a lot of sense. No, sir, he said if three wounded.` | 21 / 80.8% | 0.032864 |
| right | | `Yes, sir, wouldn't I? Oh I know. With all due respect, I don't like the constants.` | 13 / 50.0% | 0.033231 |
| side | | `I want to do respecting that.` | 1 / 3.8% | 0.001655 |
| delay-and-sum | -3 | `Yes, sir. What? With all due respect, I don't like the true world and` | 11 / 42.3% | 0.032987 |
| delay-and-sum | -2 | `Yes, sir. What? Oh, I know. With all due respect, I don't like the process. No, sir. He said, If you won't` | 19 / 73.1% | 0.032996 |
| delay-and-sum | -1 | `Yes, sir. What? Oh, I know. With all due respect, I don't like the losses. No, sir. He said, Empty wound and` | 18 / 69.2% | 0.033003 |
| delay-and-sum | 0 | Same as average | 26 / 100.0% | 0.033007 |
| delay-and-sum | +1 | `Yes, sir. What? Oh, I know. With all due respect, I don't mind a lot of seeds. No, sir, he said, if True Wound and` | 21 / 80.8% | 0.033007 |
| delay-and-sum | +2 | `Yes, sir. What? Oh, I know. With all due respect, I don't like a lot of sense. Well, sir, he said, If three wounded.` | 24 / 92.3% | 0.033004 |
| delay-and-sum | +3 | `Yes, sir, what? Oh, I know. With all due respect, I don't mind. He said, If you won't` | 16 / 61.5% | 0.032996 |

Every output still produced a TV transcript. The side channel caused the only large signal reduction. Delay-and-sum RMS stayed within 0.02% of the average, so its transcript changes do not prove acoustic suppression.

## Mixed-dialogue results

The target contains 11 normalized words. Parakeet split `TV` into `T V` in the baseline, which accounts for its two target edits. `TV marker` reports matches against the side output's opening `Yeah, chat.` marker.

| Variant | Delay | Raw transcript | Target edits / WER | TV marker | Output RMS |
|---|---:|---|---:|---:|---:|
| average | | `This is Josh speaking over the T V. One, two, three, four.` | 2 / 18.2% | 0 / 2 | 0.034082 |
| left | | Same as average | 2 / 18.2% | 0 / 2 | 0.034510 |
| right | | Same as average | 2 / 18.2% | 0 / 2 | 0.033756 |
| side | | `Yeah, chat. This is Josh Giggy from the T V one, two, and three, four, five.` | 8 / 72.7% | 2 / 2 | 0.001907 |
| delay-and-sum | -3 | Same as average | 2 / 18.2% | 0 / 2 | 0.034050 |
| delay-and-sum | -2 | Same as average | 2 / 18.2% | 0 / 2 | 0.034063 |
| delay-and-sum | -1 | Same as average | 2 / 18.2% | 0 / 2 | 0.034074 |
| delay-and-sum | 0 | Same as average | 2 / 18.2% | 0 / 2 | 0.034082 |
| delay-and-sum | +1 | Same as average | 2 / 18.2% | 0 / 2 | 0.034087 |
| delay-and-sum | +2 | Same as average | 2 / 18.2% | 0 / 2 | 0.034087 |
| delay-and-sum | +3 | Same as average | 2 / 18.2% | 0 / 2 | 0.034086 |

The baseline already omitted the TV dialogue. No delay variant could show an insertion reduction on this clip. The side output exposed two likely TV words while damaging the user text.

## Quiet-user results

The quiet reference contains 21 normalized words. The average omitted the final count and decoded `test` as `text`, for six edits.

| Variant | Delay | Raw transcript | Target edits / WER | Output RMS |
|---|---:|---|---:|---:|
| average | | `This is Josh speaking for the dictate voice text. The blue notebook is beside the window.` | 6 / 28.6% | 0.029074 |
| left | | Same as average | 6 / 28.6% | 0.029224 |
| right | | Same as average | 6 / 28.6% | 0.029168 |
| side | | `This is Josh speaking from the dictating voice text. The logo book is beside me now. One, two, three, four.` | 8 / 38.1% | 0.002663 |
| delay-and-sum | -3 | Same as average | 6 / 28.6% | 0.029080 |
| delay-and-sum | -2 | Same as average | 6 / 28.6% | 0.029084 |
| delay-and-sum | -1 | Same as average | 6 / 28.6% | 0.029082 |
| delay-and-sum | 0 | Same as average | 6 / 28.6% | 0.029074 |
| delay-and-sum | +1 | Same as average | 6 / 28.6% | 0.029056 |
| delay-and-sum | +2 | Same as average | 6 / 28.6% | 0.029033 |
| delay-and-sum | +3 | Same as average | 6 / 28.6% | 0.029009 |

## Constrained-cancellation follow-up

The next probe kept average as the target path and used side only as a bounded frequency-domain cancellation reference. It added:

- a canonical content score beside literal WER;
- a 1,024-point STFT with a 512-sample hop;
- exact passthrough below 1.5 kHz and above 7.5 kHz;
- complex least-squares mid/side weights capped at magnitude 1, limiting equal-independent-channel noise gain to 3.0103 dB over average;
- an absolute -80 dBFS calibration-side floor and 0.80 magnitude-squared coherence gate;
- pre-ASR acoustic validation with a required 3 dB reduction;
- an explicit average fallback and typed JSON failure.

The first 250 Hz fit found only three usable bands, all between 6.75 and 7.5 kHz. Most lower bands either lacked coherence or needed a raw side weight above the noise-gain cap. This rejected frequency-pooled fixed cancellation without decoding a candidate transcript.

A second implementation removed frequency pooling and fit one coefficient per 46.875 Hz FFT bin. It fit the first half of `tv-only.wav`, froze the coefficients, and tested them on the second half before ASR. No bin met both the 0.80 fit-coherence rule and the 3 dB validation-reduction rule. Several high-frequency bins had strong fit coherence but increased residual power in validation. Calibration side RMS also fell from 0.004208 in the fit half to 0.001089 in the validation half.

This result shows that the existing TV clip cannot calibrate a frozen canceller. It does not yet distinguish a changing stereo TV field from a quiet or inactive validation half. The probe therefore emitted exact average as `fallback_to_average`; no constrained candidate reached ASR, and no user score was attributed to a failed transform.

### Controlled mono calibration

After explicit consent, the fixed-geometry follow-up recorded:

- `/tmp/dictate-channel-probe/cancellation-noise.wav`: 20 seconds of room noise;
- `/tmp/dictate-channel-probe/cancellation-mono-a.wav`;
- `/tmp/dictate-channel-probe/cancellation-mono-b.wav`;
- `/tmp/dictate-channel-probe/cancellation-mono-c.wav`.

All files were native 48 kHz stereo float WAVs. The TV played continuous mono pink noise at fixed volume, and neither the TV nor laptop moved.

The capture path did not preserve a steady calibration signal. In the first 20-second attempt, A measured -29.50 dBFS over its first ten seconds and -50.54 dBFS over its last ten. A separate repeat measured -30.25 and -50.07 dBFS. Shortening the captures did not remove the behavior: the first eight-second file measured -26.20 dBFS over its first four seconds and -50.37 dBFS over its last four, while the two immediately following files measured -50.78 and -50.71 dBFS overall. PipeWire exposed the selected endpoint as the two-channel `Ryzen HD Audio Controller Digital Microphone` ALSA source and listed no filter node, but the evidence does not locate the time-varying stage more precisely.

The 20-second controlled fit found zero bins satisfying the 0.80 coherence and held-out 3 dB reduction rules. These recordings hold source, level, and geometry fixed, so their roughly 20 dB temporal change rejects the stable linear transfer required by this fixed canceller. Weakening the gates would fit capture-path adaptation rather than a repeatable TV direction.

No constrained output reached ASR. The typed failure selected exact average as designed. Running the held-out language gate on that fallback would only rescore the production baseline, so no new nearby-user or mixed-dialogue recordings were requested.

## Evidence gate

### Literal WER correction

The 25% user-only scores for left, right, and several delay variants came from rendering the five spoken numbers as digits or as `12345`. They do not show deleted speech. The mixed baseline's 18.2% score likewise came from `TV` becoming `T V`. Future spatial work must report a canonical content score beside literal WER and use number-free scripts where possible.

### Findings that remain valid

- Raw side extraction is unsafe. It reduced TV-only overlap from 26 words to 1, but it also changed quiet-user words and inserted likely TV words into the mixed transcript. Since side is `(left - right) / 2`, it cancels the centered user by construction.
- Left, right, and integer delay-and-sum did not prove TV suppression. Their output RMS stayed near the average, every TV-only output still transcribed dialogue, and the overlap score used the average variant's ASR output rather than a human TV reference.
- The mixed baseline already omitted the TV dialogue, so it offered no insertion headroom for the delay variants.
- Choosing a delay from the shortest TV-only transcript would tune steering with ASR content from one clip.

No tested transform both proved TV attenuation and preserved target speech under an independent language measure. Production averaging stays unchanged.

## Closed scope

Plan 009 settles channel selection, raw side extraction, integer-delay beamforming, and bounded fixed mid/side cancellation for this capture path. None proved TV attenuation while preserving target speech under the declared gates.

Future spatial work needs a new premise rather than looser thresholds. A new plan could start from a stable raw stereo endpoint or evaluate an explicitly time-varying method, with the same centered-speech, noise-gain, acoustic-holdout, and held-out-language protections. Production microphone capture, channel averaging, routing, and ordinary WAV transcription remain unchanged.
