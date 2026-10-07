# Mobile message read-aloud

Settings → 화면 설정 → 모바일 읽어주기 버튼. The device-local toggle defaults
to off. It does not publish an event or change desktop preferences.

When enabled, channel and thread messages show a speaker below the body on the
right. Tap it to read that message. It becomes a square stop control; the adjacent
pause/resume control preserves position. Stop clears position, so replay starts
at the beginning. Long-press menus retain their existing behavior.

The message gets an accent border while active. Actual native speech ranges are
underlined/tinted in the existing laid-out text, preserving Markdown, link taps,
wrapping, and text scaling. Pausing retains the highlight. Resume starts at the
last reported word start, so a partially spoken word may repeat rather than be
skipped. Android below API 26 and engines without word callbacks use short
chunks (up to 80 UTF-16 code units) and resume the current chunk. The UI says it
is waiting for word-position information instead of inventing moving progress.

One message speaks at a time. Changing messages cancels the previous one. Speech
stops on conversation navigation, message edit/disposal, community transition,
or disabling the toggle. Backgrounding/interruption pauses; resuming the app does
not automatically restart speech. Recording, voice-note playback, and huddles
take audio ownership only after speech stops and releases native focus.

## Free, replaceable speech

`SpeechEngine` is the replaceable interface, injected by `speechEngineProvider`.
The initial implementation uses MIT-licensed `flutter_tts` 4.2.5 with Android
TextToSpeech / iOS AVSpeechSynthesizer and `audio_session` for focus and release.
It uses no paid API, API key, Buzz endpoint or uploaded audio. Replacing the
backend requires implementing prepare/speak/stop and preserving the UTF-16
range/cancellation contract; the message UI stays the same.

Android selects only installed voices explicitly marked as not requiring a
network; unknown/network voices are not fallback candidates. Korean text selects
a Korean voice; Latin-only text selects English, otherwise the UI locale is used.
Missing voices produce an installation instruction. Voice models are not bundled
in the APK: the standard OS API works across phones, but installed engines,
languages, offline support, pronunciation, and range callbacks vary. A replacement
phone may need its Korean offline voice installed in system TTS settings.

References: <https://pub.dev/packages/flutter_tts>,
<https://developer.android.com/reference/android/speech/tts/Voice>,
<https://developer.apple.com/documentation/avfaudio/avspeechsynthesizer>.

## Validation

Run the entire mobile suite and analyzer using the repository-pinned Flutter:

```sh
flutter analyze
flutter test --dart-define=BUZZ_PUSH_GATEWAY_URL=https://push.example
```

Tests cover the actual message controls, preference persistence, pause/resume,
message switching and stale callbacks, long text, cancellation during setup,
missing/offline voices, actual rendered-text range mapping, horizontal clipping,
native method-channel start failures, late initialization, focus release and
voice-note audio handoff. Native device validation remains required for the
installed engine's audio quality, word callbacks, phone/headphone interruptions,
and iOS audio behavior. A Windows APK build cannot verify iOS runtime behavior.

Manual check on Android/iOS: start disabled; enable in Settings; play a long
Korean message in both a channel and thread; pause and resume; stop/replay;
switch messages rapidly; scroll/wrap text; leave the conversation; disable the
setting; retry without an installed Korean voice; test with downloaded voices
offline; transition to a voice note, recording, or huddle. Confirm other audio
is restored after stop and no automatic speech starts after interruptions.
