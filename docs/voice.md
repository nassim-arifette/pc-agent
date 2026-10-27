# Local voice dictation

The Windows composer accepts microphone dictation without sending audio to a provider.
Click **Dictate locally**, speak, then **Finish dictation** (or press **Ctrl+Shift+M** in the
composer). Live hypotheses appear separately from the existing draft. The completed text is
appended to that draft, never sent automatically. Edit it and click **I have reviewed this text**
before using the ordinary Send action and its existing C5 preview.

**Discard dictation** restores the pre-dictation draft. Leaving the conversation, replacing the
composer, closing the window or exiting cancels capture. The host and page reject obsolete
session IDs; monotonically increasing event sequence numbers reject out-of-order updates.

## Privacy and lifecycle

- PCM exists only in the capture queue, a bounded utterance buffer and the recognizer's memory.
  No audio file, recording, audio history or transcript log is created.
- Only settings and the downloaded model persist under the app's local data directory,
  in `voice/`. Settings contain the engine, language and an opaque microphone identifier.
- No audio crosses the Tauri bridge. Native events contain state, text, language, level and
  categorized errors only. Recognition does not construct an HTTP client.
- The only voice network operation downloads a fixed public model after separate explicit
  consent. It has no credentials, user identifiers, audio or transcription in its request.
- The text becomes an ordinary question only through the existing explicit Send path. Existing
  conversation retention then applies to that question, not to its source audio.
- The GUI and recognition worker run in the same **unelevated** process. This feature does not
  use `pcagent-elevated` and does not weaken the host's elevation guard.

## Engines and microphones

The primary backend is `whisper-rs = 0.14.4` (whisper.cpp), CPU-only, using the multilingual
Whisper **base** model. French, English and automatic language detection are selectable.
Each inference starts without prior recognition context; native logging and realtime printing
are disabled. Cancellation aborts inference using the library's safe callback.

Capture uses `cpal = 0.15.3` and the Windows default input configuration. Signed/unsigned
integer and floating-point samples are sanitized and converted to mono 16 kHz, with filtering
when downsampling. RMS supplies the level meter. Microphone selection is independent of the
system default. Windows desktop microphone privacy settings still apply.

There is **no session-duration cap**. Processing uses short utterances (silence boundaries, or
12-second work units) and a bounded queue to avoid accumulating unbounded audio. Continuous
dictation can span any number of work units. If the CPU cannot keep up, capture stops with
`audio_overrun` rather than silently dropping speech. Completed text remains available for review.

### Windows fallback: deliberately not cloud dictation

Automatic mode uses Whisper when its model exists; otherwise it attempts an **in-process
Microsoft desktop SAPI** recognizer. The explicit Windows engine uses the same offline path.
It loads a free-dictation grammar, disables adaptation/audio retention and supplies PCM through
an anonymous memory stream. It never uses the shared recognizer or Windows.Media web dictation.

This fallback is conditional, not an equivalent universal replacement for Whisper:

- A compatible Microsoft offline recognizer with a free-dictation grammar must be installed.
  Some Windows editions, including Server installations, do not provide one.
- The selected French/English language must be installed. SAPI's automatic setting selects an
  installed FR/EN recognizer; **it does not detect the language of the audio**.
- If unavailable, the app reports `engine_unavailable` and recommends downloading Whisper.
  It does not enable online Windows speech services or silently upload audio.
- A corrupt installed Whisper model produces `model_invalid`; it is not silently bypassed.

Linux/macOS retain portable settings/model/unit-test code, but native capture returns
`unsupported`. This is a build-compatible stub, not a claim of a microphone port.

## Model installation

Settings → **Local voice dictation** shows the engine, language, microphone, installation status,
size, checksum, download progress and deletion controls. Download consent is separate from the
microphone action. Capture, settings changes and model operations cannot overlap.

- File: `ggml-base.bin`
- Exact size: **147,951,465 bytes**
- SHA-256: `60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe`
- Pinned source: [ggerganov/whisper.cpp, revision 5359861](https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.bin)

The HTTPS response streams to a `.partial` model file. Byte count and checksum must match before
atomic installation; integrity is checked again before each Whisper session. Cancellation
interrupts even a stalled network future, removes the partial model, and releases the operation
slot after cleanup. Connection and stalled-read timeouts detect failures; there is no total
download-duration cap. Delete/re-download recovers an invalid model.

## IPC

Commands use the Tauri JavaScript bridge's camelCase argument names:

| Command | Arguments / return |
| --- | --- |
| `voice_start` | `{ consent: true }` → `VoiceState` |
| `voice_stop` | `{ sessionId, cancel }` → `VoiceState` |
| `voice_state` | latest `VoiceState` |
| `voice_devices` | local `{ id, name, is_default }[]` |
| `voice_settings_get` | `{ engine, language, device_id }` |
| `voice_settings_set` | `{ settings: { engine, language, device_id } }` |
| `voice_model_state` | name, bytes, SHA-256, source URL, installed/downloading/progress/error |
| `voice_model_download` | `{ consent: true }` |
| `voice_model_cancel` | cancel the active download |
| `voice_model_delete` | delete the installed model |

`voice:state` carries `{ session_id, sequence, phase, engine, language, level, text, partial,
error }`. Phases are `idle`, `starting`, `listening`, `finalizing`, `review`, `cancelled`, `error`.
`voice:model` carries the model state and progress. The frontend never invokes the conversation
Send command from an event handler. Disk/device calls and native workers do not run on the
window's event thread.

## Building and validating

Windows needs Visual Studio C++ build tools, CMake and libclang for whisper.cpp/bindgen. Put
CMake on `PATH` and set `LIBCLANG_PATH` to the directory containing `libclang.dll` where needed.
Keep `CARGO_TARGET_DIR` outside the checkout. No external executable is needed for recognition.

```powershell
cargo test -p pcagent-voice --locked
cargo clippy -p pcagent-voice -p pcagent-gui --all-targets --locked -- -D warnings
node --experimental-default-type=module --test apps/gui/dev/voice.test.mjs
```

Run the complete local gate and standard-user procedure in [validation.md](validation.md), not
just these focused tests. Synthetic tests cover resampling, silence, RMS, utterance boundaries,
digest verification, cancellation, operation exclusion, stale events, draft preservation,
review-before-send and mock model lifecycle.

The design mock generates **synthetic text**, never reads a browser microphone and never
downloads a model. Switches: `voiceModel=missing`; `voice=no_device`, `permission_denied`,
`device_unavailable`, or `engine_unavailable`. Language/engine settings affect its session.
This mock cannot prove native capture or recognition quality.

Native acceptance must use the release GUI under a standard Windows account (medium integrity,
not elevated), with FR/EN speech, cancellation, microphone disconnection/permission errors,
model download/delete and offline recognition. Use a physical microphone or a trusted signed
virtual cable fed with an inspected WAV; label the latter explicitly as **virtual-device
validation**, never as physical-microphone validation. Record exact OS/engine/device evidence and
unavailable SAPI language packs rather than treating a missing engine as a successful fallback.
