let input = Buffer.alloc(0);

process.stdin.on("data", (chunk) => {
  input = Buffer.concat([input, chunk]);
  parseAvailableMessages();
});

function parseAvailableMessages() {
  while (true) {
    const newline = input.indexOf(0x0a);
    if (newline === -1) {
      return;
    }
    const line = input.subarray(0, newline).toString("utf8").trim();
    input = input.subarray(newline + 1);
    if (line.length > 0) {
      handle(JSON.parse(line));
    }
  }
}

function handle(request) {
  if (request.id === undefined) {
    return;
  }

  if (request.method === "initialize") {
    respond(request.id, {
      protocolVersion: "2025-06-18",
      capabilities: {
        tools: {},
      },
      serverInfo: {
        name: "runx-rust-mcp-fixture",
        version: "0.0.0",
      },
    });
    return;
  }

  if (request.method === "tools/list") {
    respond(request.id, {
      tools: [
        {
          name: "echo",
          description: "Echo a message through the fixture MCP server.",
          inputSchema: {
            type: "object",
            properties: {
              message: {
                type: "string",
                description: "Message to echo.",
              },
            },
            required: ["message"],
            additionalProperties: false,
          },
        },
        {
          name: "env",
          description: "Return a single fixture server environment variable.",
          inputSchema: {
            type: "object",
            properties: {
              name: {
                type: "string",
              },
            },
            required: ["name"],
            additionalProperties: false,
          },
        },
      ],
    });
    return;
  }

  if (request.method === "tools/call") {
    handleToolCall(request.id, request.params);
    return;
  }

  respondError(request.id, -32601, "method not found");
}

function handleToolCall(id, params) {
  if (!isRecord(params) || typeof params.name !== "string") {
    respondError(id, -32602, "invalid tool call");
    return;
  }

  const args = isRecord(params.arguments) ? params.arguments : {};

  if (params.name === "env") {
    respond(id, {
      content: [
        {
          type: "text",
          text: String(process.env[String(args.name ?? "")] ?? ""),
        },
      ],
    });
    return;
  }

  if (params.name !== "echo") {
    respondError(id, -32601, "tool not found");
    return;
  }

  respond(id, {
    content: [
      {
        type: "text",
        text: String(args.message ?? ""),
      },
    ],
  });
}

function respond(id, result) {
  write({
    jsonrpc: "2.0",
    id,
    result,
  });
}

function respondError(id, code, message) {
  write({
    jsonrpc: "2.0",
    id,
    error: {
      code,
      message,
    },
  });
}

function write(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

function isRecord(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
