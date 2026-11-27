# Configs

The YAML files are retained from the Python release so existing mini-SWE-agent
configs continue to work.

- `mini.yaml`: default config for `mini` and the interactive agent.
- `default.yaml`: default config for the minimal default agent / `hello-world`.
- `mini_textbased.yaml`: text-action variant.
- `benchmarks/swebench.yaml`: SWE-bench batch runner.
- `benchmarks/programbench.yaml`: ProgramBench batch runner.

`model_prices_and_context_window.json` is a compact LiteLLM-compatible model
metadata snapshot used for context windows, reasoning levels, and cost tracking.
