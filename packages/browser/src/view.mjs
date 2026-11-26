// A run's tab, live, for the person (control.md §12). runx's host reads it
// and sends the person's input through the executor's own socket, so it
// reaches the person wherever runx does and listens nowhere else. It streams
// the tab over CDP, so it works headed, headless or attached.

const KEYS = {
  Enter: 13, Backspace: 8, Tab: 9, Escape: 27, Delete: 46,
  ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39, ArrowDown: 40,
};

/** The socket's `/view/<scope>` routes over the executor's sessions. */
export function liveView(sessions) {
  const streams = new Map();
  // Each tab's frames count on across its streams, so a reader's `after`
  // stays meaningful when a stream that went unread starts again.
  const counted = new Map();

  async function streamFor(scope, session) {
    const tab = session.driver.page;
    const open = streams.get(scope);
    // The view follows the tab the run works: a popup, then its opener again.
    if (open?.tab === tab) return open;
    if (open) end(scope);
    const client = await session.context.newCDPSession(tab);
    const stream = { tab, client, frame: undefined, waiters: new Set(), unread: 0, pressed: false };
    client.on("Page.screencastFrame", ({ data, metadata, sessionId }) => {
      void client.send("Page.screencastFrameAck", { sessionId }).catch(() => {});
      const number = (counted.get(scope) ?? 0) + 1;
      counted.set(scope, number);
      stream.frame = { number, data, metadata };
      if (stream.waiters.size) {
        stream.unread = 0;
        for (const wake of stream.waiters) wake();
        stream.waiters.clear();
      } else if (++stream.unread >= 2) {
        // Two frames nobody read: no one is watching.
        end(scope);
      }
    });
    session.context.once("close", () => { end(scope); counted.delete(scope); });
    streams.set(scope, stream);
    await client.send("Page.startScreencast", { format: "jpeg", quality: 70 });
    return stream;
  }

  function end(scope) {
    const stream = streams.get(scope);
    if (!stream) return;
    streams.delete(scope);
    for (const wake of stream.waiters) wake();
    void stream.client.send("Page.stopScreencast").catch(() => {});
    void stream.client.detach().catch(() => {});
  }

  // The frame after `after`, as soon as the tab repaints, until the reader
  // hangs up or the wait it stated passes: then the frame it had.
  async function frameAfter(scope, session, after, wait, reader) {
    const until = Date.now() + wait;
    for (;;) {
      const stream = await streamFor(scope, session);
      if (stream.frame && (stream.frame.number > after || Date.now() >= until)) return stream.frame;
      await new Promise(resolve => {
        // Until the tab's first frame there is nothing to answer with.
        const timer = stream.frame && setTimeout(resolve, Math.max(0, until - Date.now()));
        stream.waiters.add(() => { clearTimeout(timer); resolve(); });
        reader.gone.then(resolve);
      });
      if (reader.left) return undefined;
    }
  }

  async function dispatch(stream, event) {
    const { client, frame } = stream;
    if (!frame) throw Error("No frame has been seen yet. Read the view first.");
    const x = Number(event.x) * frame.metadata.deviceWidth;
    const y = Number(event.y) * frame.metadata.deviceHeight;
    const mouse = params => client.send("Input.dispatchMouseEvent", { x, y, ...params });
    switch (event.type) {
      case "down":
        stream.pressed = true;
        await mouse({ type: "mouseMoved" });
        return mouse({ type: "mousePressed", button: "left", buttons: 1, clickCount: 1 });
      case "move":
        return mouse({ type: "mouseMoved", ...(stream.pressed && { button: "left", buttons: 1 }) });
      case "up":
        stream.pressed = false;
        return mouse({ type: "mouseReleased", button: "left", buttons: 0, clickCount: 1 });
      case "wheel":
        return mouse({ type: "mouseWheel", deltaX: Number(event.dx) || 0, deltaY: Number(event.dy) || 0 });
      case "text":
        return client.send("Input.insertText", { text: String(event.text).slice(0, 64) });
      case "key": {
        const code = KEYS[event.key];
        if (!code) throw Error("Unknown key.");
        const key = { key: event.key, code: event.key, windowsVirtualKeyCode: code };
        const enter = event.key === "Enter";
        await client.send("Input.dispatchKeyEvent", {
          ...key, type: enter ? "keyDown" : "rawKeyDown", ...(enter && { text: "\r" }),
        });
        return client.send("Input.dispatchKeyEvent", { ...key, type: "keyUp" });
      }
      default:
        throw Error("Unknown input.");
    }
  }

  /** Answer `GET /view/<scope>?after=<n>` or `POST /view/<scope>/input`. */
  return async function serve(req, res) {
    // One framing for runx's host to read: a length, never chunks.
    const send = (code, value) => {
      const body = JSON.stringify(value);
      res.writeHead(code, {
        "content-type": "application/json", "cache-control": "no-store", "content-length": Buffer.byteLength(body),
      });
      res.end(body);
    };
    const url = new URL(req.url ?? "/", "http://executor");
    const [, scope, input] = url.pathname.match(/^\/view\/([a-f0-9]{64})(\/input)?$/) ?? [];
    if (!scope) return send(404, { refused: "Unknown view request." });
    const session = sessions.get(scope);
    try {
      if (req.method === "GET" && !input) {
        if (!session) return send(200, { live: false });
        const number = name => {
          const value = Number(url.searchParams.get(name) ?? 0);
          return Number.isSafeInteger(value) && value >= 0 ? value : 0;
        };
        const after = number("after");
        const reader = { left: false };
        reader.gone = new Promise(resolve => res.once("close", () => { reader.left = true; resolve(); }));
        const frame = await frameAfter(scope, session, after, number("wait"), reader);
        if (!frame) return;
        return send(200, {
          live: true,
          url: session.driver.page.url(),
          title: await session.driver.page.title().catch(() => ""),
          frame: frame.number,
          ...(frame.number > after && { image: frame.data }),
          width: frame.metadata.deviceWidth,
          height: frame.metadata.deviceHeight,
        });
      }
      if (req.method === "POST" && input) {
        if (!session) return send(409, { refused: "This run has no browser open." });
        let body = "";
        for await (const chunk of req) {
          body += chunk;
          if (body.length > 4096) return send(413, { refused: "Input is too large." });
        }
        const event = JSON.parse(body);
        const stream = await streamFor(scope, session);
        session.driver.attend();
        await dispatch(stream, event);
        return send(200, { ok: true });
      }
      return send(405, { refused: "Read a view with GET, give input with POST." });
    } catch (error) {
      return send(409, { refused: error instanceof SyntaxError ? "Input isn't JSON." : error.message });
    }
  };
}
