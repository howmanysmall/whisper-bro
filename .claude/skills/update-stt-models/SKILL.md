---
name: update-stt-models
description: Refresh the README's transcription model picks from the Artificial Analysis leaderboard.
disable-model-invocation: true
---

Refresh the "Pick a transcription model" section of `README.md` so its three picks (best performance, best value, best overall) match today's leaderboard and today's OpenRouter catalog.

## Steps

1. **Read the leaderboard** at <https://artificialanalysis.ai/speech-to-text/non-streaming> with Claude in Chrome. Load the `claude-in-chrome` skill first. The charts show 35 models by default, so open the WER chart's model selector and pick **Select all** before reading. The "Summary of Key Metrics" table at the bottom lists every API-benchmarked model with word error rate, median speed factor, and USD per 1,000 minutes. Use that table as the data source. Done when you have WER, speed, and price for every row.
2. **Filter to OpenRouter.** The utility posts to `https://openrouter.ai/api/v1/audio/transcriptions`. A model qualifies only if OpenRouter serves it there:

   ```sh
   curl -s "https://openrouter.ai/api/v1/models?output_modalities=transcription" | jq -r '.data[] | [.id, .name] | @tsv'
   ```

   Match leaderboard names to OpenRouter IDs by model name and version. A different version is a different model: Grok STT 1.0 is not Grok Voice Transcribe 2.0. Chat models that accept audio don't qualify. Done when each leaderboard row is either matched to an ID or dropped.
3. **Choose the picks** from the matched rows:
   - **Best performance**: lowest word error rate.
   - **Best value**: lowest price among models with usable speed and WER. Cheapest wins unless its WER is far worse than a slightly pricier model.
   - **Best overall**: the best balance of WER, speed, and price. When one model leads on WER and sits near the top for speed and price, it takes both performance and overall.
   Also note the runner-up for performance and any better model that OpenRouter lacks.
4. **Check prices** on the OpenRouter model page (`https://openrouter.ai/<id>`) for each pick. The raw `pricing.prompt` from the API uses inconsistent units across models, so trust the page's displayed price. Note when it differs from the leaderboard's.
5. **Update `README.md`**: rewrite the table and the prose under the transcription model heading. Set the last-checked date to today in `YYYY-MM-DD`. Keep the vocabulary caveat: OpenRouter forwards `vocabulary` only to Groq and DeepInfra. If a pick replaces the default model, mention it to the user rather than editing `src/config.rs`.
6. **Lint** with `mise exec -- vale README.md` and fix every error and warning in the section. Suggestion-level alerts on product names and the category labels can stay. Done when the section shows only suggestions.
7. **Report** the picks that changed, with their old and new numbers, to the user.
