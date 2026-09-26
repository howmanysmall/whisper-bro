# Whisper Bro

I wanted voice typing for a coworker who uses long spoken prompts, including in apps that don't have a microphone button. This is a small background utility inspired by [Wispr Flow](https://wisprflow.ai/): press a shortcut, talk, and insert the finished text where you were typing.

The speech is streamed to [Deepgram Nova-3](https://developers.deepgram.com/docs/live-streaming-audio) while you talk. Optional cleanup uses [GPT OSS 20B on Groq](https://console.groq.com/docs/model/openai/gpt-oss-20b). Both use your own API keys and are billed separately from ChatGPT or Claude subscriptions. I haven't established real-world accuracy or latency numbers yet.

## Run it

Development needs [Mise](https://mise.jdx.dev/), plus Xcode Command Line Tools on macOS or the Visual Studio C++ build tools on Windows. `mise.toml` pins Rust; `Cargo.lock` pins dependencies.

```sh
mise trust
mise install
mise exec -- cargo run -- setup
mise run dev
```

Get a key from [Deepgram](https://console.deepgram.com/). Setup stores it in macOS Keychain or Windows Credential Manager. Supplying a [Groq key](https://console.groq.com/keys) enables cleanup; leaving it blank keeps the current cleanup setting. `DEEPGRAM_API_KEY` and `GROQ_API_KEY` environment variables take precedence over stored keys.

Click a text field, press **Ctrl+Alt+Space**, speak, then press the same shortcut again. **Escape** cancels recording or finalization. The tray/menu-bar menu has recording controls, recovery, and Quit. Leave the destination focused while the result finishes; if focus changes, recover it using **Copy last dictation**.

On macOS, use the packaged app for a stable permission identity. Grant Microphone and Accessibility access to Whisper Bro in **System Settings → Privacy & Security**. Development runs may attribute permissions to the terminal instead. Restart after changing Accessibility permission.

On Windows, enable microphone access for desktop apps. Normal desktop apps cannot inject input into an elevated app; in that case, recover the text from the tray and paste it yourself.

## Configure it

`whisper-bro config` prints the configuration path and effective settings. Edit that TOML file and restart the utility. For example:

```toml
shortcut = "Ctrl+Alt+Space"
mode = "hold"
sounds = true
language = "en"
vocabulary = ["Luau", "roblox-ts", "T3 Chat"]
# microphone = "Your microphone's exact name"

[cleanup]
enabled = true
model = "openai/gpt-oss-20b"
timeout_ms = 1200
```

`toggle` mode is the default because holding a key through a long prompt gets old. `hold` records from press to release. The shortcut accepts Ctrl, Alt, Shift, Cmd/Win, letters, digits, Space, Backspace, and F1–F12. Letter shortcuts refer to physical US-layout key positions on macOS.

Use `whisper-bro devices` to get microphone names. Leaving `microphone` unset follows the system default at the beginning of each recording. A disconnected or stalled device stops that recording; reconnect it and start another.

Vocabulary is passed through Deepgram's [keyterm prompting](https://developers.deepgram.com/docs/keyterm), which is a separately priced feature. Keep it short: the service limits the combined terms to 500 tokens. Cleanup preserves the complete speech transcript if its request times out, fails, or produces truncated output. It doesn't hold the result indefinitely waiting for an edit.

Other commands:

```sh
whisper-bro doctor
whisper-bro last
whisper-bro last --raw
whisper-bro last --copy
whisper-bro autostart enable
whisper-bro autostart disable
whisper-bro --config /path/to/config.toml run
```

Use the installed executable when enabling login startup, so moving a development build doesn't break the saved path. On macOS, that is `/Applications/Whisper Bro.app/Contents/MacOS/whisper-bro`.

## Recover a result

The latest nonempty result is saved before insertion. Failed recordings can replace it with their confirmed partial transcript, marked incomplete. `last` reports that condition on stderr. Cancelling deliberately discards the active recording.

The recovery file contains both the speech transcript and the cleaned text. `doctor` prints its directory. Only the last result is kept; audio stays in memory. Delete `last-dictation.json` to remove the saved text. Logs contain timing and error diagnostics rather than transcript bodies or API keys.

Clipboard insertion snapshots the prior clipboard and restores it after a short paste delay, provided another app hasn't changed it in the meantime. Clipboard read timing differs between apps, so a delayed paste is one of the things to check when validating a new destination. The utility inserts text without pressing Enter.

Speech requests use Deepgram's [`mip_opt_out=true`](https://developers.deepgram.com/docs/the-deepgram-model-improvement-partnership-program) option. When cleanup is enabled, the transcript also goes to Groq.

## Build and check

```sh
mise run check
mise run build
mise run package:macos
# On Windows, with Inno Setup 6 on PATH:
mise run package:windows
```

The macOS package is written under `dist/`. Local bundles are ad-hoc signed. For distribution, set `MACOS_SIGNING_IDENTITY` to an installed Developer ID certificate; set `APPLE_NOTARY_PROFILE` to an existing notarytool keychain profile to notarize and staple. Windows packaging produces a per-user installer with a setup shortcut.

CI runs formatting, Clippy, and tests on macOS and Windows, packages each native build, and cross-builds an Intel Mac bundle. These checks don't replace testing microphone permissions, hotkeys, and insertion in real applications.

## Measure the part that matters

The useful number is the time between stopping and having text in the destination. Streaming does most recognition during recording, and [`CloseStream`](https://developers.deepgram.com/docs/close-stream) flushes the remaining audio immediately. The client waits for the final metadata acknowledgement rather than guessing when the last word arrived.

For repeatable provider checks, use the same WAV recordings:

```sh
whisper-bro transcribe --realtime prompt.wav
whisper-bro transcribe --realtime --no-cleanup prompt.wav
```

Text goes to stdout; connection, finalization, and total result timings go to stderr. Normal dictation writes microphone-ready and stop-to-paste timings to `whisper-bro.log` in the directory reported by `doctor`.

Initial acceptance targets are 150 ms to recording-ready feedback and, for 5–60-second dictations on a healthy connection, 1 second median / 2 seconds p95 from stop to paste. Those are targets, not measured claims. Check T3 Chat, Discord, and a code editor on both operating systems with short sentences, technical names, spoken corrections, multi-minute prompts, focus changes, network loss, and headset reconnects. Include cold connections and Bluetooth microphone startup in the measurements.
