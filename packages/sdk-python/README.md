# runx-py

Python SDK for [runx](https://runx.ai) — the governed runtime for agent skills, tools, and graphs.

`runx-py` is a thin Python client over the `runx` CLI JSON output. Install the CLI separately (`@runxhq/cli` on npm), then use this package from Python to search and run skills, continue runs awaiting agent input, and format host protocol results for popular agent frameworks.

## Rust takeover boundary

`runx-py` remains a thin client over the `runx` CLI JSON contract after the
Rust takeover. CLI JSON output preservation keeps this package working through
the cutover.

See the [TypeScript interop boundary](../../docs/ts-interop-boundary.md) for
the package disposition and ownership rules.

## Install

```bash
pip install runx-py
```

You will also need the `runx` CLI on your `PATH`:

```bash
npm install -g @runxhq/cli
```

## Usage

```python
from runx import RunxClient

client = RunxClient()

# Search the registry
for result in client.search_skills("sourcey"):
    print(result.skill_id, result.version)

# Run a skill
report = client.run_skill("skills/sourcey", inputs={"project": "."})
print(report["status"])

continued = client.continue_run("skills/sourcey", run_id="run_123", answers_file="answers.json")
print(continued["status"])
```

Non-string JSON values are encoded as JSON text rather than Python `str()`
representations, including booleans, `None`, objects, and arrays. Ordinary
strings remain verbatim for compatibility: a string that itself contains valid
JSON, such as `"true"` or `"42"`, is still interpreted by the CLI's JSON-first
parser. Non-JSON objects use their string projection; non-finite floats and
integers outside the CLI's numeric range are not guaranteed to round-trip.

## Framework adapters

Bridge runx into an existing agent framework (OpenAI, Anthropic, CrewAI, LangChain, Vercel AI):

```python
from runx import create_host_bridge, create_openai_host_adapter

bridge = create_host_bridge(run=my_host_run, continue_run=my_host_continue)
adapter = create_openai_host_adapter(bridge)
response = adapter.run("skills/sourcey")
```

The bridge translates host protocol results, including `needs_agent` runs and approval gates, into framework-native tool messages. `RunxClient` remains a CLI client; host protocol execution is provided by the embedding runtime.

## Links

- Homepage: <https://runx.ai>
- Documentation: <https://runx.ai/docs>
- Source: <https://github.com/runxhq/runx>
- Issues: <https://github.com/runxhq/runx/issues>

## Releasing

See [RELEASING.md](RELEASING.md) for the automated tag-driven publish flow.

## License

Apache-2.0
