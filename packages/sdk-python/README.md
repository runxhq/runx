# runx-py

Python SDK for [runx](https://runx.ai), the governed runtime for agent skills, tools, and graphs.

`runx-py` is a thin Python client over the `runx` CLI JSON output. Install the CLI separately (`@runxhq/cli` on npm), then use this package from Python to search and run skills, continue runs awaiting agent input, and format host protocol results for popular agent frameworks.

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
for result in client.search_skills("web search"):
    print(result.skill_id, result.version, result.install_command.copy_text)

# Run a skill: each input is passed as -i name=value and read by the type the
# runner declares.
report = client.run_skill("runx/web-search", inputs={"query": "receipt verification"})
print(report["status"])

continued = client.continue_run("runx/web-search", run_id="run_123", answers_file="answers.json")
print(continued["status"])
```

## Framework adapters

Bridge runx into an existing agent framework (OpenAI, Anthropic, CrewAI, LangChain, Vercel AI):

```python
from runx import create_host_bridge, create_openai_host_adapter

bridge = create_host_bridge(run=my_host_run, continue_run=my_host_continue)
adapter = create_openai_host_adapter(bridge)
response = adapter.run("runx/web-search")
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
