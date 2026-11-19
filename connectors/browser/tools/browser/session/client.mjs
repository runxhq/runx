// A trusted Runx process operation. The caller cannot choose a socket, CDP
// endpoint or browser profile. The native host_browser binding names the profile.
import { lstat, readFile, realpath } from "node:fs/promises";
import { request } from "node:http";
import { join } from "node:path";

const input = JSON.parse(await readFile(process.env.RUNX_INPUTS_PATH, "utf8"));
const profile = input.parameters?.profile;
if (typeof profile !== "string" || !/^[a-z0-9][a-z0-9_-]{0,47}$/.test(profile))
  throw Error("Invalid native browser binding.");
// The tool starts with a scrubbed environment. Use the same fixed rendezvous
// directory as the executor and trust only a socket owned by this user.
const uid = process.getuid();
const root = join(await realpath("/tmp"), "runx-browser-" + uid);
const socketPath = join(root, profile + ".sock");
const directory = await lstat(root).catch(() => null);
const socket = await lstat(socketPath).catch(() => null);
if (!directory?.isDirectory() || directory.uid !== uid || directory.mode & 0o077 ||
    !socket?.isSocket() || socket.uid !== uid)
  throw Error("The bound browser executor is unavailable.");
const operation =
  input.connector_operation ?? (input.operation === "session" ? "session" : "");
const job = request(
  {
    socketPath,
    path: "/" + operation,
    method: "POST",
    headers: { "content-type": "application/json" },
  },
  async (res) => {
    let body = "";
    for await (const chunk of res) {
      body += chunk;
      if (body.length > 262144)
        throw Error("Browser response exceeds the limit.");
    }
    const value = JSON.parse(body);
    if (res.statusCode === 200 || res.statusCode === 409) {
      process.stdout.write(JSON.stringify(value));
    } else {
      process.stderr.write(
        "The browser could not confirm the operation. Inspect site state before retrying.\n",
      );
      process.exitCode = 1;
    }
  },
);
job.on("error", () => {
  process.stderr.write("The bound browser executor is unavailable.\n");
  process.exitCode = 1;
});
job.setTimeout(25000, () => job.destroy());
job.end(JSON.stringify(input));
