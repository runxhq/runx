# runx-py

Python SDK for [runx](https://runx.ai), the governed runtime for agent skills, tools, and graphs.

`runx-py` is a thin Python client over the `runx` CLI's JSON output. Install the CLI separately (`@runxhq/cli` on npm), then use this package from Python to search the registry, run skills, and resume a paused run with a `runx.resume.v1` answers document.

See the [TypeScript interop boundary](../../docs/ts-interop-boundary.md) for
the package outcome and ownership rules.

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

# A run that waits on a person or an agent returns its report too, and its
# run_id resumes it with a runx.resume.v1 answers document.
resumed = client.resume(report["run_id"], answers_file="answers.json")
print(resumed["status"])
```

## Links

- Homepage: <https://runx.ai>
- Documentation: <https://runx.ai/docs>
- Source: <https://github.com/runxhq/runx>
- Issues: <https://github.com/runxhq/runx/issues>

## Releasing

See [RELEASING.md](RELEASING.md) for the automated tag-driven publish flow.

## License

Apache-2.0
