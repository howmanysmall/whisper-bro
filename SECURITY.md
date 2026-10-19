# Security policy

## Reporting a vulnerability

Report vulnerabilities through [GitHub private vulnerability reporting](https://github.com/howmanysmall/yap/security/advisories/new).
Please do not open a public issue for a security problem.

## Supported versions

Only the latest release is supported.

## Data flow

- Audio and transcripts are sent to OpenRouter and the providers it routes to.
- Your API key is stored in the macOS Keychain or the Windows Credential Manager.
- The recovery file on disk is plaintext.
- Yap has no telemetry.
