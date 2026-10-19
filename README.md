# Yap

I wanted voice typing for a coworker who uses long spoken prompts, including in apps that don't have a microphone button. This is a small background utility inspired by [Wispr Flow](https://wisprflow.ai/): press a shortcut, talk, and insert the finished text where you were typing. It runs on macOS and Windows.

When you stop, the recording goes to [OpenRouter](https://openrouter.ai/) for transcription with Whisper Large v3 Turbo. Optional cleanup runs the transcript through GPT OSS 20 B, also on OpenRouter. Both use your own OpenRouter key. OpenRouter bills that account, which is separate from any ChatGPT or Claude plan. I haven't established real-world accuracy or latency numbers yet.

## Install

### With mise

```sh
mise use -g github:howmanysmall/yap
```

Run `mise up` to upgrade. Mise verifies each download against the release's `SHA256SUMS.txt` and build provenance attestations. This installs the `yap` command. It doesn't install the macOS app or the Windows installer.

### Manual downloads

Each [release](https://github.com/howmanysmall/yap/releases) has these files:

- `Yap-macos-universal-app.zip`: one app for Apple Silicon and Intel.
- `Yap-windows-x64-setup.exe`: a per-user installer.
- `yap-x86_64-pc-windows-msvc.zip`: the portable Windows build.

None of them carry a real signature yet, so the first launch is blocked.

- On macOS 15 and later, open System Settings → Privacy & Security, scroll to the message about Yap, and choose **Open Anyway**. Right-click → **Open** no longer works.
- On Windows, SmartScreen blocks the installer. Choose "More info → Run anyway."

### First run

1. Get a key from [OpenRouter](https://openrouter.ai/settings/keys).
2. Start Yap. The packaged app asks for the key at launch when none is stored. You can also choose **Set API key…** in the tray or menu-bar menu, or run `yap setup` in a terminal.
3. Setup stores the key in macOS Keychain or Windows Credential Manager. Pressing Enter at the key prompt keeps the stored key. The next prompt turns cleanup on with `y`, and pressing Enter there turns cleanup off. An `OPENROUTER_API_KEY` environment variable takes precedence over the stored key.
4. On macOS, grant Microphone and Accessibility access to Yap in **System Settings → Privacy & Security** when the prompts appear. Restart Yap after you change the Accessibility permission. Use the packaged app for a stable permission identity. A run from a terminal may attribute permissions to the terminal instead.
5. On Windows, enable microphone access for desktop apps.
6. To start Yap at login, choose **Start at login** in the menu or run `yap autostart enable`. When mise installed Yap, the login item launches mise's `yap` shim, so it survives `mise up`.

Upgrading from Whisper Bro: Yap copies the stored key, `config.toml`, and the last dictation from the old locations the first time it runs. It leaves the old files and key in place. Running `yap autostart enable` or `yap autostart disable` removes the old Whisper Bro login item, so you end up with a single entry.

## Use it

Click a text field, press **Ctrl+Alt+Space**, speak, then press the same shortcut again. Yap pastes the text without pressing Enter. Leave the destination focused while the result finishes.

In `hold` mode, you hold the shortcut while you talk and release it to finish. The default is `toggle`, because holding a key through a long prompt gets old. See [Configure](#configure) to change either.

**Escape** cancels, but only while Yap is recording. Cancelling discards the recording.

### What you see and hear

- On macOS, the menu bar shows a microphone icon that turns red while recording. A small HUD under the menu bar stays up while Yap records and shows a timer such as `Recording 0:42`.
- On Windows, the tray shows a dot. It is grey when idle, red while recording, amber while Yap transcribes, and shows "!" after an error.
- Each stage has its own sound. Turn them off with **Play sounds** or `sounds = false`.
- At startup, Yap shows "Ready · $X.XX credits" with your remaining OpenRouter credit. It warns you when credit runs low and when OpenRouter rejects the key.

### The menu

Click the menu-bar or tray icon for these items:

- **Start recording** / **Stop recording**: the same as the shortcut.
- **Cancel dictation**: the same as Escape.
- **Copy last dictation**: puts the last result on the clipboard.
- **Retry last recording**: uploads a recording that failed to transcribe. See [Recover and retry](#recover-and-retry).
- **Set API key…**: replaces the stored key without a restart.
- **Open config file** and **Open log folder**.
- **Clean up transcript**, **Play sounds**, and **Hold to talk**: switch the matching settings. Each one rewrites `config.toml`.
- **Start at login**: the same switch as `yap autostart enable` and `yap autostart disable`.
- **Check for updates…**: opens the releases page.
- **Quit Yap**.

If pasting fails, the text stays on the clipboard and in the recovery file. This happens when focus moves to another window while Yap works. Use **Copy last dictation** or `yap last` to get it back.

On Windows, a normal app can't type into an elevated app. If the target runs as administrator, recover the text from the tray and paste it yourself.

## Configure

`yap config` prints the configuration path and effective settings. Edit that TOML file and restart Yap. This example shows every setting at its default, except that `microphone` is commented out because it has no default:

```toml
shortcut = "Ctrl+Alt+Space"
mode = "toggle" # or "hold"
sounds = true
language = "en" # or "auto" to let the model detect it
vocabulary = [] # for example ["Luau", "roblox-ts", "T3 Chat"]
# microphone = "Your microphone's exact name"
max_recording_seconds = 600
silence_timeout_seconds = 30
paste_restore_ms = 200
transcription_model = "openai/whisper-large-v3-turbo"
transcription_timeout_ms = 60000

[cleanup]
enabled = false
model = "openai/gpt-oss-20b"
timeout_ms = 8000
# instructions = "Write numbers as digits."
replace_instructions = false
```

Yap rejects a file with unknown keys or out-of-range values and names the problem.

**Shortcut and mode.** The shortcut accepts Ctrl, Alt, Shift, Cmd/Win, letters, digits, Space, Backspace, and F1 to F12. Escape is reserved for cancel. Letter shortcuts refer to physical US-layout key positions on macOS. `toggle` starts and stops with the same press. `hold` records from press to release.

**Language and vocabulary.** `language` is a language code, or `"auto"` to let the model detect it. Yap joins `vocabulary` into a prompt that shows the transcription model how to spell those words. It takes up to 100 terms. OpenRouter passes the prompt on only when it routes the request to Groq or DeepInfra. Other providers ignore it.

**Microphone.** Run `yap devices` to list microphone names. Leaving `microphone` unset follows the system default at the start of each recording. A disconnected or stalled device stops that recording. Reconnect it and start another.

**Recording limits.** Recording stops on its own after `max_recording_seconds`, from 1 to 3600. It also stops after `silence_timeout_seconds` without speech. A recording with no speech is never uploaded.

**Upload.** Yap resamples the recording to 16 kHz mono before upload, which is what the transcription models use anyway. Recordings up to 25 MB upload as a plain multipart file. Longer ones fall back to a base64 JSON body. `transcription_model` accepts any OpenRouter audio transcription model. `transcription_timeout_ms` limits how long the upload and transcription may take, from 1000 to 120000.

**Pasting.** `paste_restore_ms` is how long Yap waits after pasting before it restores the previous clipboard, from 50 to 5000. Raise it for apps that paste late.

**Cleanup.** `cleanup.timeout_ms` is how long Yap waits for cleanup before pasting the unedited transcript, from 100 to 60000. The 8000 default leaves room for long spoken prompts. `cleanup.instructions` adds your own guidance after the built-in cleanup prompt. Set `replace_instructions = true` to use only your text.

**Moving the config file.** `--config <path>` moves only the config file. Recovery, logs, and the instance lock stay in the data directory that `yap doctor` prints.

The tray toggles for cleanup, sounds, and hold mode rewrite the file. Comments in it may not survive.

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

## Recover and retry

Yap saves each nonempty result before inserting it. The recovery file contains both the speech transcript and the cleaned text. It stores only one result, and `yap doctor` prints its directory. Audio stays in memory unless its upload fails.

- If the microphone fails partway, Yap still transcribes and saves the audio recorded until then. It marks that result incomplete and doesn't insert it. `yap last` reports that on stderr.
- If an upload fails with a server error, a dropped connection, or a timeout, Yap tries once more a second later.
- If that fails too, or OpenRouter rejects the request outright, Yap keeps the recording as `last-recording.wav` until the next successful dictation and keeps the previous result.

Retry a kept recording with **Retry last recording** in the menu or with `yap last --retry`.

`yap last` prints the cleaned text. `yap last --raw` prints the transcript before cleanup. `yap last --copy` puts the text on the clipboard instead of printing it.

Clipboard insertion snapshots the prior clipboard and restores it after a short paste delay, provided another app hasn't changed it in the meantime. Apps read the clipboard at different times. When you try a new app, check that the paste doesn't arrive late.

### Privacy

The recording goes to OpenRouter, which forwards it to a transcription provider. With cleanup on, the transcript takes the same route to a language model. Your [OpenRouter privacy settings](https://openrouter.ai/settings/privacy) decide whether those providers may log or train on your data.

The recovery file is plaintext. Delete `last-dictation.json` to remove the saved text. Logs contain timing and error diagnostics rather than transcript bodies or API keys. Yap sends no telemetry. When Yap writes a result to the clipboard, it marks the write transient, so clipboard managers that honor the mark skip it.

## Troubleshooting

Run `yap doctor` first. It prints the version, the config path and whether it parses, the data directory, whether Yap is running, the key status, a microphone probe, and permission status.

- **Nothing pastes, or Yap says "The destination changed".** Focus moved to another window while Yap worked. The text is on the clipboard. Paste it, or run **Copy last dictation**.
- **macOS asks for Accessibility again after an update.** Local and release bundles carry an ad-hoc signature, so macOS treats each build as a new app. Remove Yap from the Accessibility list in System Settings → Privacy & Security, add it again, and restart Yap.
- **Recording captures nothing on macOS.** Microphone access is denied, and macOS then delivers silence. Allow Yap in System Settings → Privacy & Security → Microphone.
- **"Another app already registered" the shortcut.** Another program owns the key combination. Change `shortcut` in `config.toml`. On macOS, Ctrl+Alt+Space can also collide with an input-source switch shortcut. Change one of the two.
- **Yap can't paste into one Windows app.** That app probably runs as administrator. Recover the text from the tray and paste it yourself.
- **"OpenRouter rejected the API key".** Choose **Set API key…** or run `yap setup` with a new key. Check the `OPENROUTER_API_KEY` environment variable too, because it overrides the stored key.
- **"OpenRouter account has no credits".** Add credit at OpenRouter. Then use **Retry last recording**.
- **`config.toml` is broken.** Every command still works, and Yap falls back to defaults where it can. `yap doctor` names the error. Fix the file, or delete it to start over.
- **A clipboard manager shows or misses dictations.** Yap marks its clipboard writes as transient. Managers that ignore the mark still record them. Pause the manager or exclude Yap.
- **Uploads fail behind a corporate proxy.** Yap uses a bundled set of root certificates. A TLS-intercepting proxy presents its own certificate, which Yap doesn't trust. Ask your administrator to exempt `openrouter.ai`.
- **Where is the log?** In the data directory that `yap doctor` prints, as `yap.log`. Set `RUST_LOG=debug` for more detail.

## Uninstall

### macOS

1. Run `yap autostart disable`, or delete `~/Library/LaunchAgents/dev.yap.plist`.
2. Quit Yap, then delete `Yap.app`. If you installed with mise, run `mise uninstall github:howmanysmall/yap`.
3. Delete the data directory that `yap doctor` prints.
4. Open Keychain Access and delete the item named `dev.yap`.

### Windows

1. Run `yap autostart disable`, or delete the `Yap` value under `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`.
2. Quit Yap, then run the uninstaller from Settings → Apps. For the portable build, delete its folder.
3. Delete the data directory that `yap doctor` prints.
4. Open Credential Manager and delete the entry named `dev.yap`.

## Commands

Run `yap` with no command to start the utility. These commands work in a terminal:

- `yap run`: start the tray or menu-bar utility. This is the default.
- `yap setup`: store the API key in the OS credential store and create `config.toml`.
- `yap devices`: list microphone names for the `microphone` setting.
- `yap config`: print the config path, then the effective settings.
- `yap last [--raw|--copy|--retry]`: show the last dictation. `--raw` shows the transcript before cleanup, `--copy` copies it, and `--retry` first uploads the recording kept by a failed transcription.
- `yap transcribe <file> [--no-cleanup]`: transcribe a WAV file without inserting text anywhere.
- `yap autostart enable|disable`: turn login startup on or off. The change applies at the next login.
- `yap doctor`: show configuration, key status, and permission status.
- `yap --config <path> <command>`: use a different config file with any command.

## Develop

Development needs [Mise](https://mise.jdx.dev/), plus Xcode Command Line Tools on macOS or the Visual Studio C++ build tools on Windows. `mise.toml` pins Rust, and `Cargo.lock` pins dependencies.

```sh
mise trust
mise install
mise exec -- cargo run -- setup
mise run dev
```

Use the installed executable when enabling login startup, so moving a development build doesn't break the saved path. On macOS, that's `/Applications/Yap.app/Contents/MacOS/yap`.

### Measure the part that matters

The useful number is the time between stopping and having text in the destination. Yap uploads the whole recording when you stop, so that time grows with the length of the recording and your upload speed.

For repeatable provider checks, use the same WAV recordings:

```sh
yap transcribe prompt.wav
yap transcribe --no-cleanup prompt.wav
```

Text goes to stdout, and timings go to stderr. `finalize` is the upload and transcription time, and `stop_to_result` adds cleanup. Normal dictation writes microphone-ready and stop-to-paste timings to `yap.log`. Set `RUST_LOG=debug` to raise the log level.

The first targets are 150 ms to recording-ready feedback and 1 second median from stop to paste. The p95 target is 2 seconds. Both paste targets apply to dictations of 5 to 60 seconds on a stable connection. Nobody has measured them yet, and they date from the streaming design, so the upload design may need new ones. Check T3 Chat, Discord, and a code editor on both operating systems with short sentences, technical names, spoken corrections, multi-minute prompts, focus changes, network loss, and headset reconnects. Include cold connections and Bluetooth microphone startup in the measurements.

## Build and package

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

The release has `yap-<triple>.tar.gz` (the bare binary, which mise installs) next to the downloads listed under [Install](#install).
