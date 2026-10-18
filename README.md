# Whisper Bro

I wanted voice typing for a coworker who uses long spoken prompts, including in apps that don't have a microphone button. This is a small background utility inspired by [Wispr Flow](https://wisprflow.ai/): press a shortcut, talk, and insert the finished text where you were typing.

When you stop, the recording goes to [OpenRouter](https://openrouter.ai/) for transcription with Whisper Large v3 Turbo. Optional cleanup runs the transcript through GPT OSS 20 B, also on OpenRouter. Both use your own OpenRouter key and are billed to that account, not to a ChatGPT or Claude subscription. I haven't established real-world accuracy or latency numbers yet.

## Run it

Development needs [Mise](https://mise.jdx.dev/), plus Xcode Command Line Tools on macOS or the Visual Studio C++ build tools on Windows. `mise.toml` pins Rust, and `Cargo.lock` pins dependencies.

```sh
mise trust
mise install
mise exec -- cargo run -- setup
mise run dev
```

Get a key from [OpenRouter](https://openrouter.ai/settings/keys). Setup stores it in macOS Keychain or Windows Credential Manager. Pressing Enter at the key prompt keeps the stored key. The next prompt turns cleanup on with `y`, and pressing Enter there turns cleanup off. An `OPENROUTER_API_KEY` environment variable takes precedence over the stored key.

Click a text field, press **Ctrl+Alt+Space**, speak, then press the same shortcut again. **Escape** cancels recording or transcription. The tray/menu-bar menu has recording controls, recovery, and Quit. Leave the destination focused while the result finishes; if focus changes, recover it using **Copy last dictation**.

On macOS, use the packaged app for a stable permission identity. Grant Microphone and Accessibility access to Whisper Bro in **System Settings → Privacy & Security**. Development runs may attribute permissions to the terminal instead. Restart after changing Accessibility permission.

On Windows, enable microphone access for desktop apps. Normal desktop apps cannot inject input into an elevated app; in that case, recover the text from the tray and paste it yourself.

## Configure it

`whisper-bro config` prints the configuration path and effective settings. Edit that TOML file and restart the utility:

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

Use `whisper-bro devices` to get microphone names. Leaving `microphone` unset follows the system default at the beginning of each recording. A disconnected or stalled device stops that recording. Reconnect it and start another. Recording stops on its own after `max_recording_seconds`.

`transcription_model` accepts any OpenRouter audio transcription model. `transcription_timeout_ms` (1000 to 120000) limits how long the upload and transcription may take.

Vocabulary is joined into a prompt that tells the transcription model how to spell those words. OpenRouter passes it on only when it routes the request to Groq or DeepInfra. Other providers ignore it.

Cleanup is off by default. When on, it fixes punctuation and capitalization, removes filler, repetitions, and false starts, and applies spoken self-corrections ("Friday, actually Monday" becomes "Monday") before insertion. It keeps names, numbers, and technical identifiers as spoken, and doesn't summarize or answer the text. It keeps the original transcript if its request times out, fails, or produces truncated output. It doesn't hold the result indefinitely waiting for an edit.

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

The latest nonempty result is saved before insertion. If the microphone fails partway, the audio recorded until then is still transcribed and saved, marked incomplete, but not inserted. `last` reports that condition on stderr. If transcription itself fails, the recording is lost and the previous result stays. Cancelling discards the active recording.

The recovery file contains both the speech transcript and the cleaned text. `doctor` prints its directory. Only the last result is kept, and audio stays in memory. Delete `last-dictation.json` to remove the saved text. Logs contain timing and error diagnostics rather than transcript bodies or API keys.

Clipboard insertion snapshots the prior clipboard and restores it after a short paste delay, provided another app hasn't changed it in the meantime. Clipboard read timing differs between apps, so a delayed paste is one of the things to check when validating a new destination. The utility inserts text without pressing Enter.

The recording goes to OpenRouter, which forwards it to a transcription provider; when cleanup is enabled, the transcript takes the same route to a language model. Whether those providers may log or train on your data is governed by your [OpenRouter privacy settings](https://openrouter.ai/settings/privacy), not by this utility.

## Build and check

```sh
mise run check
mise run build
mise run package:macos
# On Windows:
mise run package:windows
```

The macOS package is written under `dist/`. Local bundles are ad-hoc signed. For distribution, set `MACOS_SIGNING_IDENTITY` to an installed Developer ID certificate. To notarize and staple, set `APPLE_NOTARY_PROFILE` to an existing notarytool keychain profile. On Windows, `package:windows` writes the per-user installer and the portable `.zip` side by side; `mise run package:windows-portable` skips the installer, which is the part that needs Inno Setup 6.

Every package stamps its version from `Cargo.toml` at build time, so the app bundle and the installer can't disagree with the crate.

On Linux, CI runs the hk checks, cargo-deny's advisory and license checks, a secret scan of the full history, and commitlint on pull requests. Clippy and the tests run on macOS and Windows. Every package builds too, including an Intel Mac bundle cross-built on Apple Silicon. Each run's packages are downloadable from its summary page. These checks don't replace testing microphone permissions, hotkeys, and insertion in real applications.

`main` only accepts pull requests that pass the `CI` check. Repository admins bypass that rule, which is what lets the release command push to `main` directly.

## Release

Releases come from `main`, and one command cuts them with [bumpp](https://github.com/antfu-collective/bumpp):

```sh
mise run release         # choose the version from a prompt
mise run release patch   # 0.1.0 -> 0.1.1
```

The argument is `patch`, `minor`, `major`, or an explicit version like `0.2.0`. The command runs `mise run check`, then shows the new version and recent commits and waits for confirmation before changing anything. It bumps `package.json`, `Cargo.toml`, and `Cargo.lock` together, commits `chore(release): vX.Y.Z`, tags `vX.Y.Z`, and pushes. It refuses uncommitted changes, any branch but `main`, a `package.json` version that disagrees with `Cargo.toml`, or unpushed local tags that would go out with the release.

That tag starts the release workflow. It reruns CI and publishes every package in a GitHub release with `SHA256SUMS.txt`. While CI runs, [communique](https://github.com/jdx/communique) writes the release notes from the commits since the last tag, using `communique.toml` and the `OPENROUTER_API_KEY` repository secret. If that fails, the release falls back to GitHub's generated notes, and the run's `release-notes-review` artifact lists which commits the notes covered. A tag like `v0.2.0-rc.1` publishes as a prerelease. The release is created only after every build succeeds. A red build doesn't publish anything and leaves a tag to retry from. Re-running the workflow retries the upload, and the workflow refuses a tag that disagrees with `Cargo.toml`. If the tag itself is wrong, delete it (`git push origin :refs/tags/vX.Y.Z`) and run the release command again.

The downloads are `Whisper Bro.app` for Apple Silicon and Intel, `whisper-bro-windows-x64-setup.exe`, and `whisper-bro-windows-x64-portable.zip`. None of them are signed yet. macOS bundles come out ad-hoc signed, so open the app with right-click → **Open** the first time. On Windows, SmartScreen blocks the installer until you choose "More info → Run anyway."

## Measure the part that matters

The useful number is the time between stopping and having text in the destination. The utility uploads the whole recording when you stop, so that time grows with the length of the recording and your upload speed.

For repeatable provider checks, use the same WAV recordings:

```sh
whisper-bro transcribe --realtime prompt.wav
whisper-bro transcribe --realtime --no-cleanup prompt.wav
```

Text goes to stdout, and timings go to stderr. `finalize` is the upload and transcription time, and `stop_to_result` adds cleanup. `connect` always reads `0ms`, because there is no streaming connection any more. Normal dictation writes microphone-ready and stop-to-paste timings to `whisper-bro.log` in the directory reported by `doctor`.

Initial acceptance targets are 150 ms to recording-ready feedback and, for dictations of 5 to 60 seconds on a stable connection, 1 second median and 2 seconds p95 from stop to paste. Nobody has measured them yet, and they date from the streaming design, so the upload design may need new ones. Check T3 Chat, Discord, and a code editor on both operating systems with short sentences, technical names, spoken corrections, multi-minute prompts, focus changes, network loss, and headset reconnects. Include cold connections and Bluetooth microphone startup in the measurements.
