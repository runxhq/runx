# @runxhq/langchain

Wrap a finite governed Runx skill invocation as a LangChain tool. The host
supplies an `@runxhq/client` instance and `runOptions`, including the skill path,
receipt directory and optional execution binding.

```ts
import { createRunxLangChainTool } from "@runxhq/langchain";
const tool = createRunxLangChainTool({
  name: "research", description: "Run bounded research.", schema,
  client: runx,
  runOptions: { skillPath: "/skills/research", receiptDir: "/work/receipts" },
});
```

`mapInput` maps the tool input to the skill input object. `formatOutput` can
project a successful native result. Pending resolutions, payment, denied runs
and sealed failures reject the finite tool call; the owning host handles them.
This package never spawns a process, implements an agent loop or resumes an
approval on the host's behalf. Native invocation belongs to `@runxhq/client`.
