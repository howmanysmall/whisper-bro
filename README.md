# Yap

I wanted voice typing for a coworker who uses long spoken prompts, including in apps that don't have a microphone button. This is a small background utility inspired by [Wispr Flow](https://wisprflow.ai/): press a shortcut, talk, and insert the finished text where you were typing.

When you stop, the recording goes to [OpenRouter](https://openrouter.ai/) for transcription with Whisper Large v3 Turbo. Optional cleanup runs the transcript through GPT OSS 20 B, also on OpenRouter. Both use your own OpenRouter key. OpenRouter bills that account, which is separate from any ChatGPT or Claude plan. I haven't established real-world accuracy or latency numbers yet.

## Run it

Development needs [Mise](https://mise.jdx.dev/), plus Xcode Command Line Tools on macOS or the Visual Studio C++ build tools on Windows. `mise.toml` pins Rust, and `Cargo.lock` pins dependencies.

```sh
mise trust
mise install
mise exec -- cargo run -- setup
mise run dev
```

Get a key from [OpenRouter](https://openrouter.ai/settings/keys). Setup stores it in macOS Keychain or Windows Credential Manager. Pressing Enter at the key prompt keeps the stored key. The next prompt turns cleanup on with `y`, and pressing Enter there turns cleanup off. An `OPENROUTER_API_KEY` environment variable takes precedence over the stored key.

Upgrading from Whisper Bro: Yap copies the stored key, `config.toml`, and the last dictation from the old locations the first time it runs, and it leaves the old files and key in place. Running `yap autostart enable` or `yap autostart disable` removes the old Whisper Bro login item, so you end up with a single entry.

Click a text field, press **Ctrl+Alt+Space**, speak, then press the same shortcut again. **Escape** cancels recording or transcription. The tray/menu-bar menu has recording controls, recovery, and Quit. Leave the destination focused while the result finishes. If focus changes, recover it using **Copy last dictation**.

On macOS, use the packaged app for a stable permission identity. Grant Microphone and Accessibility access to Yap in **System Settings → Privacy & Security**. Development runs may attribute permissions to the terminal instead. Restart after changing Accessibility permission.

On Windows, enable microphone access for desktop apps. Normal desktop apps can't type into an elevated app. In that case, recover the text from the tray and paste it yourself.

## Configure it

`yap config` prints the configuration path and effective settings. Edit that TOML file and restart the utility:

```toml
shortcut = "Ctrl+Alt+Space"
mode = "hold"
sounds = true
language = "en"
vocabulary = ["Luau", "roblox-ts", "T3 Chat"]
# microphone = "Your microphone's exact name"
max_recording_seconds = 600
transcription_model = "openai/whisper-large-v3-turbo"
transcription_timeout_ms = 60000

[cleanup]
enabled = true
model = "openai/gpt-oss-20b"
timeout_ms = 1200
```

`toggle` mode is the default because holding a key through a long prompt gets old. `hold` records from press to release. The shortcut accepts Ctrl, Alt, Shift, Cmd/Win, letters, digits, Space, Backspace, and F1 to F12. Letter shortcuts refer to physical US-layout key positions on macOS.

Use `yap devices` to get microphone names. Leaving `microphone` unset follows the system default at the beginning of each recording. A disconnected or stalled device stops that recording. Reconnect it and start another. Recording stops on its own after `max_recording_seconds`.

`transcription_model` accepts any OpenRouter audio transcription model. `transcription_timeout_ms` limits how long the upload and transcription may take, from 1000 to 120000.

The utility joins `vocabulary` into a prompt that shows the transcription model how to spell those words. OpenRouter passes it on only when it routes the request to Groq or DeepInfra. Other providers ignore it.

### Pick a transcription model

Last checked 2026-10-18 against the [Artificial Analysis non-streaming leaderboard](https://artificialanalysis.ai/speech-to-text/non-streaming). The utility uploads the finished recording, so the streaming board doesn't apply.

A model is eligible when OpenRouter's transcription endpoint serves it through the provider the leaderboard benchmarked, and its word error rate is 5% or lower. Fourteen models qualify. Each category ranks them by its own rule:

- **Best performance**: lowest word error rate. Ties go to the faster model.
- **Best value**: lowest price. Ties go to the lower error rate, then the faster model.
- **Best overall**: lowest sum of the model's three ranks among eligible models, one each for error rate, speed, and price. Ties go to the lower error rate.

Word error rate is AA-WER v2, where lower is better. Speed is seconds of audio transcribed per second, so 317× turns a minute of speech into text in under 0.2 seconds. Prices are what OpenRouter charges for the benchmarked provider, in USD per 1,000 minutes of audio.

#### Best performance

| Rank | Model              | `transcription_model`          | Word error rate | Speed vs. real time | USD per 1,000 minutes |
| ---- | ------------------ | ------------------------------ | --------------- | ------------------- | --------------------- |
| 1    | MAI-Transcribe-2   | `microsoft/mai-transcribe-2`   | 2.0%            | 317×                | $1.67                 |
| 2    | Scribe v2          | `elevenlabs/scribe-v2`         | 2.2%            | 63×                 | $1.83                 |
| 3    | MAI-Transcribe-1.5 | `microsoft/mai-transcribe-1.5` | 2.4%            | 81×                 | $6.00                 |

#### Best value

| Rank | Model                          | `transcription_model`            | Word error rate | Speed vs. real time | USD per 1,000 minutes |
| ---- | ------------------------------ | -------------------------------- | --------------- | ------------------- | --------------------- |
| 1    | Whisper Large v3 Turbo on Groq | `openai/whisper-large-v3-turbo`  | 4.6%            | 195×                | $0.67                 |
| 2    | Voxtral Mini on DeepInfra      | `mistralai/voxtral-mini-3b-2507` | 3.8%            | 51×                 | $1.00                 |
| 3    | Parakeet TDT 0.6B v3           | `nvidia/parakeet-tdt-0.6b-v3`    | 4.5%            | 527×                | $1.50                 |

#### Best overall

| Rank | Model                | `transcription_model`         | Rank sum | Word error rate | Speed vs. real time | USD per 1,000 minutes |
| ---- | -------------------- | ----------------------------- | -------- | --------------- | ------------------- | --------------------- |
| 1    | MAI-Transcribe-2     | `microsoft/mai-transcribe-2`  | 7        | 2.0%            | 317×                | $1.67                 |
| 2    | Parakeet TDT 0.6B v3 | `nvidia/parakeet-tdt-0.6b-v3` | 15       | 4.5%            | 527×                | $1.50                 |
| 3    | Scribe v2            | `elevenlabs/scribe-v2`        | 16       | 2.2%            | 63×                 | $1.83                 |

MAI-Transcribe-2 leads overall because it places first on error rate, second on speed, and fourth on price. Parakeet runs at 527×, ahead of every other eligible model, but ranks near the bottom on accuracy. Scribe v2 comes within 0.2 points of MAI-Transcribe-2's error rate but runs at a fifth of its speed.

Whisper Large v3 Turbo stays the default as the value pick. OpenRouter also routes it to DeepInfra at $0.20, which the leaderboard hasn't benchmarked. Switching away from Whisper drops `vocabulary`, since OpenRouter passes the prompt only to Groq and DeepInfra.

Some models fall outside the rules. Alibaba's Fun-Realtime-ASR-preview has the lowest error rate on the leaderboard at 1.7%, but OpenRouter doesn't serve it. OpenRouter serves Voxtral Small only through DeepInfra, while the leaderboard benchmarked Mistral's own API. Deepgram Nova-3 makes 5.2% errors.

Other commands:

```sh
yap doctor
yap last
yap last --raw
yap last --copy
yap autostart enable
yap autostart disable
yap --config /path/to/config.toml run
```

Use the installed executable when enabling login startup, so moving a development build doesn't break the saved path. On macOS, that's `/Applications/Yap.app/Contents/MacOS/yap`.

## Recover a result

Yap resamples the recording to 16 kHz mono before upload, which is what the transcription models use anyway. The utility saves each nonempty result before inserting it. If the microphone fails partway, the utility still transcribes and saves the audio recorded until then. It marks that result incomplete and doesn't insert it. `last` reports that condition on stderr. If transcription itself fails, the utility loses the recording and keeps the previous result. Cancelling discards the active recording.

The recovery file contains both the speech transcript and the cleaned text. `doctor` prints its directory. The file stores only one result, and audio stays in memory. Delete `last-dictation.json` to remove the saved text. Logs contain timing and error diagnostics rather than transcript bodies or API keys.

Clipboard insertion snapshots the prior clipboard and restores it after a short paste delay, provided another app hasn't changed it in the meantime. Apps read the clipboard at different times. When you try a new app, check that the paste doesn't arrive late. The utility inserts text without pressing Enter.

The recording goes to OpenRouter, which forwards it to a transcription provider. With cleanup on, the transcript takes the same route to a language model. Your [OpenRouter privacy settings](https://openrouter.ai/settings/privacy) decide whether those providers may log or train on your data.

## Build and check

```sh
mise run check
mise run build
mise run package:macos
# On Windows:
mise run package:windows
```

The macOS package lands under `dist/`. Local bundles get an ad-hoc signature. For distribution, set `MACOS_SIGNING_IDENTITY` to an installed Developer ID certificate. To notarize and staple, set `APPLE_NOTARY_PROFILE` to an existing notarytool keychain profile. On Windows, `package:windows` writes the per-user installer and the portable `.zip` side by side. `mise run package:windows-portable` skips the installer, which is the part that needs Inno Setup 6.

Every package stamps its version from `Cargo.toml` at build time, so the app bundle and the installer can't disagree with the crate.

On Linux, CI runs the hk checks, cargo-deny's advisory and license checks, a secret scan of the full history, and commitlint on pull requests. Clippy and the tests run on macOS and Windows. Every package builds too, including an Intel Mac bundle cross-built on Apple Silicon. Each run's packages are downloadable from its summary page. These checks don't replace testing microphone permissions, hotkeys, and insertion in real applications.

`main` only accepts pull requests that pass the `CI` check. Repository admins bypass that rule, which is what lets the release command push to `main` directly.

## Release

Releases come from `main`, and one command cuts them with [bumpp](https://github.com/antfu-collective/bumpp):

```sh
mise run release         # choose the version from a prompt
mise run release patch   # 0.1.0 -> 0.1.1
```

The argument is `patch`, `minor`, `major`, or an explicit version like `0.2.0`. The command runs `mise run check` first. Then it shows the new version and recent commits, and waits for you to confirm before it changes anything. It bumps `package.json`, `Cargo.toml`, and `Cargo.lock` together, commits `chore(release): vX.Y.Z`, tags `vX.Y.Z`, and pushes. It stops if you have uncommitted changes or aren't on `main`. It also stops if `package.json` and `Cargo.toml` disagree on the version, or if unpushed local tags would go out with the release.

That tag starts the release workflow. It reruns CI and publishes every package in a GitHub release with `SHA256SUMS.txt`. While CI runs, [communique](https://github.com/jdx/communique) writes the release notes from the commits since the last tag, using `communique.toml` and the `OPENROUTER_API_KEY` repository secret. If that fails, the release falls back to GitHub's generated notes, and the run's `release-notes-review` artifact lists which commits the notes covered. A tag like `v0.2.0-rc.1` publishes as a prerelease. The workflow creates the release only after every build succeeds. A red build doesn't publish anything and leaves a tag to retry from. Re-running the workflow retries the upload, and the workflow refuses a tag that disagrees with `Cargo.toml`. If the tag itself is wrong, delete it with `git push origin :refs/tags/vX.Y.Z` and run the release command again.

The downloads are `yap-macos-universal.zip` (one app for Apple Silicon and Intel), `yap-windows-x64-setup.exe`, and `yap-windows-x64-portable.zip`. None of them carry a real signature yet. macOS bundles only have an ad-hoc signature, so the first launch is blocked. On macOS 15 and later, open System Settings → Privacy & Security, scroll to the message about Yap, and choose **Open Anyway**. Right-click → **Open** no longer works. On Windows, SmartScreen blocks the installer until you choose "More info → Run anyway."

## Measure the part that matters

The useful number is the time between stopping and having text in the destination. The utility uploads the whole recording when you stop, so that time grows with the length of the recording and your upload speed.

For repeatable provider checks, use the same WAV recordings:

```sh
yap transcribe prompt.wav
yap transcribe --no-cleanup prompt.wav
```

Text goes to stdout, and timings go to stderr. `finalize` is the upload and transcription time, and `stop_to_result` adds cleanup. Normal dictation writes microphone-ready and stop-to-paste timings to `yap.log` in the directory reported by `doctor`. Set `RUST_LOG=debug` to raise the log level.

The first targets are 150 ms to recording-ready feedback and 1 second median from stop to paste. The p95 target is 2 seconds. Both paste targets apply to dictations of 5 to 60 seconds on a stable connection. Nobody has measured them yet, and they date from the streaming design, so the upload design may need new ones. Check T3 Chat, Discord, and a code editor on both operating systems with short sentences, technical names, spoken corrections, multi-minute prompts, focus changes, network loss, and headset reconnects. Include cold connections and Bluetooth microphone startup in the measurements.
