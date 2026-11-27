# Compatibility map

This port was written against the `Wiselfx/mini-swe-agent` fork at version
2.4.6. The Python implementation was left unchanged.

| Python component | Rust component |
| --- | --- |
| `agents/default.py` | `src/agents/default.rs` |
| `agents/interactive.py` | interactive mode in `src/agents/default.rs` |
| `agents/utils/compaction.py` | `src/agents/compaction.rs` |
| `models/litellm_model.py` | `src/models/native.rs` |
| text/toolcall/Responses action utilities | `src/models/actions.rs` |
| OpenRouter/Portkey/Requesty models | provider modes in `src/models/native.rs` |
| deterministic and meta models | `src/models/deterministic.rs`, `src/models/roulette.rs` |
| local/Docker/Singularity/bubblewrap | `src/environments/` |
| ConTree | native HTTP implementation in `src/environments/contree.rs` |
| SWE-ReX | `src/environments/swerex_docker.rs`, `src/environments/swerex_modal.rs` |
| `run/mini.py` | `src/bin/mini.rs` |
| global config and sessions | `src/run/utilities/` |
| SWE-bench runners | `src/run/benchmarks/swebench.rs`, `src/bin/swebench*.rs` |
| ProgramBench runner | `src/bin/programbench.rs` |
| trajectory inspector | `src/bin/inspector.rs` |

## Preserved behavior

- Linear message history and the same system/user/assistant/tool/exit roles.
- Exactly-one-action text parsing and bash tool-call parsing.
- Responses API `function_call` / `function_call_output` items.
- Format-error feedback, repeated-format-error exit, and response/cost
  persistence on failed parses.
- Step, cost, wall-clock, and global cost/call limits.
- Resume behavior, including observations for pending tool calls.
- Compaction excerpts, gap markers, two summaries, and full compacted history.
- `COMPLETE_TASK_AND_SUBMIT_FINAL_OUTPUT` and `WAIT_FOR_USER` control flow.
- Local, Docker, Singularity, bubblewrap, and ConTree submission detection.
- Jinja-style strict templates, including `tojson`, whitespace control, and
  built-in observation truncation.
- Recursive YAML and `key.path=value` config merging.
- Atomic trajectory saves and the `mini-swe-agent-1.1` schema.
- Provider usage normalization, cache-read pricing, model profiles, reasoning
  levels, retries, and Anthropic cache-control/thinking handling.

## Deliberate language/runtime differences

- Python plugin import paths and Python bindings are replaced by Rust traits and
  the built-in class factories. Original built-in class path strings remain
  accepted for config compatibility.
- The native provider router covers the built-in providers and arbitrary
  OpenAI-compatible `api_base` servers. It is not a reimplementation of every
  provider adapter in the LiteLLM catalog.
- Modal sandbox provisioning used the Modal Python SDK. The Rust SWE-ReX Modal
  environment attaches to an existing runtime via `runtime_url` or `SWEREX_URL`.
- ProgramBench reads a local JSON/JSONL dataset from `PROGRAMBENCH_DATASET`
  instead of importing the Python `programbench` package.
- Local SWE-bench Parquet loading is not included; Hugging Face and JSON/JSONL
  dataset loading are supported.
- The terminal UI is native Ratatui/Crossterm rather than Rich/Textual. The
  inspector navigation keys and trajectory grouping are retained.
- Interactive `Ctrl+C` handling is not yet equivalent during a blocking provider
  request; normal completion, rejection, mode switching, and limit prompts are
  implemented.

## Verification

- Python baseline: 285 core agent/environment/config/utility tests passed.
- Rust: 17 behavior tests covering config/templates, local execution, agent
  submission/limits/resume, compaction, all action formats, cache control,
  OpenAI-compatible HTTP, Anthropic HTTP, Responses API, ConTree, and SWE-ReX.
- CLI smoke tests: interactive config/task/submission, resume, and local
  SWE-bench batch output including `preds.json` and trajectory metadata.
