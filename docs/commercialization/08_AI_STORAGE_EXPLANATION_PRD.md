# 08 AI Storage Explanation PRD

## Position
AI explains storage. Deterministic rules decide safety and actions.

## Use cases
1. Explain a category.
2. Explain why it grew.
3. Summarize reclaimable candidates.
4. Explain tradeoffs.
5. Suggest a cleanup sequence.

## Never allow
- AI-generated raw shell commands to execute automatically
- AI to override safety classifications
- AI to infer that a user-owned file is disposable
- cloud upload of file contents by default

## Input contract
AI receives structured facts:
- entity type
- path class, not sensitive file contents
- size
- mtime
- usage evidence
- tool
- project
- risk
- regenerability
- cleanup method

## Output contract
```json
{
  "summary": "string",
  "why_large": ["string"],
  "what_happens_if_removed": "string",
  "recommendation": "keep|review|clean",
  "confidence": 0.0
}
```

The `recommendation` is advisory only.

## Local-first
Default:
- template/rule explanation without network
Optional:
- user-enabled LLM provider
- user-selected local model

## Example
Input:
Ollama model, 42 GB, last used 92 days ago, downloadable, destructive.

Output:
> This model occupies 42 GB and has not been observed as recently used. It can be downloaded again, but removing it will require another download before use.

The UI then offers:
Keep / Archive later / Remove

## Quality metrics
- hallucination rate
- unsafe recommendation rate
- explanation usefulness
- latency
- percentage answered without network
