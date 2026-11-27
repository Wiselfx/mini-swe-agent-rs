# mini-swe-agent-rs

A native Rust rewrite of [mini-swe-agent](https://github.com/SWE-agent/mini-swe-agent),
preserving the agent loop, YAML configuration, CLI names, message formats,
trajectory schema, action parsing, model accounting, and execution semantics.

## Install

```bash
cargo install --path .
```

This installs:

- `mini` and `mini-swe-agent`: interactive/local agent CLI
- `mini-extra` and `mini-e`: global `.env` configuration utility
- `hello-world`: smallest default-agent example
- `swebench`: parallel SWE-bench batch runner
- `swebench-single`: one SWE-bench instance
- `programbench`: ProgramBench batch runner
- `inspector`: terminal trajectory browser

## Quick start

```bash
mini-extra config setup
mini -t "Fix the failing tests in this repository"
```

Or run without confirmation:

```bash
mini -y -t "Explain what this repository does"
```

Resume the last selected session, or a specific trajectory:

```bash
mini -r
mini -r -o /path/to/run.traj.json -t "Continue with the remaining edge cases"
```

Configs merge left to right. Existing YAML files and `key.path=value` specs are
supported:

```bash
mini -c mini.yaml -c model.model_kwargs.temperature=0
mini -c mini.yaml -c model.model_name=openai/gpt-5-mini
```

## Models

Native HTTP implementations are included for:

- LiteLLM-style OpenAI-compatible endpoints, including OpenAI, Gemini, Groq,
  DeepSeek, Mistral, xAI, Together, Fireworks, OpenRouter, and custom
  `model_kwargs.api_base` servers
- Anthropic Messages API, including thinking blocks and cache-control markers
- OpenRouter chat completions and Responses API
- Portkey chat completions and Responses API
- Requesty chat completions
- Text actions, chat tool calls, and Responses API function calls
- Deterministic test models, roulette, and interleaving models

Cost tracking uses the bundled model registry, `model.pricing` overrides, or
provider-reported usage. `cost_tracking: ignore_errors` remains available for
free/local models.

## Environments

- Local shell with merged stdout/stderr, inherited stdin, environment overrides,
  process-group timeout cleanup, and submission detection
- Docker/Podman
- Singularity/Apptainer
- Bubblewrap
- ConTree through its native HTTP API
- SWE-ReX Docker through direct container execution
- SWE-ReX remote runtimes through `runtime_url` / `SWEREX_URL`

The original Modal deployment provisions a sandbox through the Modal Python SDK.
The Rust build does not reproduce that SDK-specific provisioning step; point
`runtime_url` or `SWEREX_URL` at an existing SWE-ReX server instead.

## Library

```rust
use mini_swe_agent::{
    agents::DefaultAgent,
    environments::LocalEnvironment,
    models::get_model,
    Map,
};

fn main() -> mini_swe_agent::Result<()> {
    let model = get_model(Map::from_iter([
        ("model_name".into(), "openai/gpt-5-mini".into()),
    ]))?;
    let mut agent = DefaultAgent::new(
        model,
        Box::new(LocalEnvironment::new(Map::new())),
        Map::from_iter([
            ("system_template".into(), "You are a helpful assistant.".into()),
            ("instance_template".into(), "{{task}}".into()),
        ]),
        false,
    );
    agent.run("Write a sudoku game".into(), None, Map::new())?;
    Ok(())
}
```

## Compatibility notes

The Rust port keeps the `mini-swe-agent-1.1` trajectory format and accepts the
original built-in class paths in configs. Python plugin imports and Python
bindings are inherently replaced by Rust traits and built-in factories.

SWE-bench datasets load from Hugging Face or local JSON/JSONL. ProgramBench
accepts a local JSON/JSONL dataset through `PROGRAMBENCH_DATASET`. The inspector
keeps the original navigation keys and trajectory grouping.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -j 1 -- -D warnings
cargo test --tests -j 1
```

The test suite uses local mock HTTP servers; no API keys or cloud accounts are
required.

## License

MIT. See [LICENSE.md](LICENSE.md). This rewrite retains the upstream project's
attribution and license.
