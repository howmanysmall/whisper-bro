---
name: update-stt-models
description: Refresh the README's transcription model rankings from the Artificial Analysis leaderboard.
disable-model-invocation: true
---

Refresh the "Pick a transcription model" section of `README.md`. That section states the eligibility rule and the ranking rule for each category, and it is the source of truth for them. Apply those rules exactly as written. Change a rule only when the user asks.

## Steps

1. **Read the leaderboard** at <https://artificialanalysis.ai/speech-to-text/non-streaming> with Claude in Chrome. Load the `claude-in-chrome` skill first. The charts show 35 models by default, so open the WER chart's model selector and pick **Select all** before reading. The "Summary of Key Metrics" table at the bottom lists every API-benchmarked model and provider with word error rate, median speed factor, and price. Done when you have every row.
2. **List OpenRouter's transcription models and their providers.** The utility posts to `https://openrouter.ai/api/v1/audio/transcriptions`. Only those models count:

   ```sh
   curl -s "https://openrouter.ai/api/v1/models?output_modalities=transcription" | jq -r '.data[].id'
   curl -s "https://openrouter.ai/api/v1/models/<id>/endpoints" | jq -r '.data.endpoints[] | [.provider_name, (.pricing|tostring)] | @tsv'
   ```

   Match each leaderboard row to an OpenRouter ID and provider by model name and version. A different version is a different model: Grok STT 1.0 is not Grok Voice Transcribe 2.0. Done when each leaderboard row is matched or dropped with a reason.
3. **Price each match** in USD per 1,000 minutes for the benchmarked provider. A per-second `pricing.prompt` times 60,000 gives that figure. When the endpoint prices by token or by hour, read the displayed price on `https://openrouter.ai/<id>`, and fall back to the leaderboard's price for token-priced models.
4. **Rank** every eligible model under each category's rule with a script in the scratchpad, never by eye. Rank sums use dense ranks: tied values share a rank. Done when each category has its top three.
5. **Update `README.md`**: rewrite the three tables, the eligible model count, the explanation paragraphs, and the list of excluded models, and set the last-checked date to today in `YYYY-MM-DD`. Keep the vocabulary caveat: OpenRouter forwards `vocabulary` only to Groq and DeepInfra. If the value pick changes, tell the user, since `src/config.rs` defaults to it. Leave the code unchanged.
6. **Lint** with `mise exec -- vale --minAlertLevel=warning README.md` and fix every alert inside the section. Model names stay as their vendors spell them, such as the `0.6B` in Parakeet TDT 0.6B v3. Done when only model-name alerts remain.
7. **Report** to the user which models entered or left each top three, with old and new numbers.
