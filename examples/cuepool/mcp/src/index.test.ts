import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import {
  createServer,
  type IncomingMessage,
  type ServerResponse,
} from "node:http";
import { once } from "node:events";
import path from "node:path";
import { createInterface } from "node:readline";
import test from "node:test";

import { Client } from "@modelcontextprotocol/client";
import { StdioClientTransport } from "@modelcontextprotocol/client/stdio";

const READ_TOOLS = [
  "cuepool_active_cues",
  "cuepool_cues",
  "cuepool_diagnostics",
  "cuepool_health",
  "cuepool_logs",
  "cuepool_project",
];

test("read-only discovery and health call work over stdio", async (t) => {
  const api = await mockApi((request, response) => {
    assert.equal(request.url, "/v1/health");
    json(response, 200, { status: "ok", ready: true, control_enabled: false });
  });
  t.after(() => api.close());

  const { client, transport } = await mcpClient(api.url);
  t.after(() => client.close());
  t.after(() => transport.close());

  const tools = await client.listTools();
  assert.deepEqual(tools.tools.map(({ name }) => name).sort(), READ_TOOLS);

  const result = await client.callTool({
    name: "cuepool_health",
    arguments: {},
  });
  assert.equal(result.isError, undefined);
  assert.deepEqual(result.structuredContent, {
    data: { status: "ok", ready: true, control_enabled: false },
  });
});

test("logs are paged below the STDIO transport limit", async (t) => {
  const message = "x".repeat(16 * 1024);
  const api = await mockApi((request, response) => {
    assert.equal(request.url, "/v1/logs?limit=100");
    json(response, 200, {
      entries: Array.from({ length: 400 }, (_, index) => ({
        cursor: index + 1,
        recorded_at: "2026-08-14T00:00:00.000Z",
        level: "info",
        target: "cuepool",
        message,
      })),
      next_cursor: 400,
      truncated: false,
    });
  });
  t.after(() => api.close());

  const { client, transport } = await mcpClient(api.url);
  t.after(() => client.close());
  t.after(() => transport.close());

  const result = await client.callTool({
    name: "cuepool_logs",
    arguments: {},
  });
  assert.equal(result.isError, undefined);
  const logs = result.structuredContent as {
    data: { entries: unknown[]; next_cursor: number; truncated: boolean };
  };
  assert.equal(logs.data.entries.length, 100);
  assert.equal(logs.data.next_cursor, 100);
  assert.equal(logs.data.truncated, true);
});

test("large read results use structured content without overflowing STDIO", async (t) => {
  const largeName = "x".repeat(6 * 1024 * 1024);
  const api = await mockApi((request, response) => {
    assert.equal(request.url, "/v1/cues");
    assert.equal(request.headers.authorization, undefined);
    json(response, 200, [{ qid: "1", name: largeName }]);
  });
  t.after(() => api.close());

  const { client, transport } = await mcpClient(api.url, "configured-token");
  t.after(() => client.close());
  t.after(() => transport.close());

  const result = await client.callTool({ name: "cuepool_cues", arguments: {} });
  assert.equal(result.isError, undefined);
  assert.match(
    (result.content[0] as { text: string }).text,
    /See structured content/,
  );
  const cues = result.structuredContent as { data: Array<{ name: string }> };
  assert.equal(cues.data[0]?.name.length, largeName.length);
});

test("oversized API responses fail before the body is buffered", async (t) => {
  const api = await mockApi((_request, response) => {
    response.writeHead(200, {
      "Content-Type": "application/json",
      "Content-Length": String(25 * 1024 * 1024),
    });
    response.end("{}");
  });
  t.after(() => api.close());

  const { client, transport } = await mcpClient(api.url);
  t.after(() => client.close());
  t.after(() => transport.close());

  const result = await client.callTool({
    name: "cuepool_health",
    arguments: {},
  });
  assert.equal(result.isError, true);
  assert.equal(
    (result.structuredContent as { error: { kind: string } }).error.kind,
    "response_too_large",
  );
});

test("large API error messages are truncated before STDIO rendering", async (t) => {
  const api = await mockApi((_request, response) => {
    json(response, 503, {
      error: "unavailable",
      message: "x".repeat(6 * 1024 * 1024),
    });
  });
  t.after(() => api.close());

  const { client, transport } = await mcpClient(api.url);
  t.after(() => client.close());
  t.after(() => transport.close());

  const result = await client.callTool({
    name: "cuepool_health",
    arguments: {},
  });
  assert.equal(result.isError, true);
  const failure = result.structuredContent as {
    error: { message: string };
    data_omitted: boolean;
  };
  assert.match(failure.error.message, /… \[truncated\]$/);
  assert.ok(Buffer.byteLength(failure.error.message) < 17 * 1024);
  assert.equal(failure.data_omitted, true);
});

test("control tools preserve applied and rejected command results", async (t) => {
  let nextId = 6;
  const commands = new Map<number, Record<string, unknown>>();
  const api = await mockApi(async (request, response) => {
    if (request.method === "POST" && request.url === "/v1/commands") {
      assert.equal(request.headers.authorization, "Bearer test-token");
      const idempotencyKey = request.headers["idempotency-key"];
      if (typeof idempotencyKey !== "string") {
        throw new Error("missing Idempotency-Key header");
      }
      assert.match(idempotencyKey, /^test-(go|pause)-1$/);
      const command = JSON.parse(await body(request)) as { command: string };
      const id = ++nextId;
      commands.set(id, {
        id,
        state: command.command === "pause" ? "rejected" : "applied",
        message:
          command.command === "pause" ? "nothing is playing" : "GO applied",
        created_at: "2026-08-14T00:00:00.000Z",
        completed_at: "2026-08-14T00:00:00.001Z",
      });
      json(response, 202, {
        id,
        state: "pending",
        message: null,
        created_at: "2026-08-14T00:00:00.000Z",
        completed_at: null,
      });
      return;
    }
    const match = request.url?.match(/^\/v1\/commands\/(\d+)$/);
    if (request.method === "GET" && match) {
      assert.equal(request.headers.authorization, undefined);
      json(response, 200, commands.get(Number(match[1])));
      return;
    }
    json(response, 404, { error: "not_found", message: "not found" });
  });
  t.after(() => api.close());

  const { client, transport } = await mcpClient(api.url, "test-token");
  t.after(() => client.close());
  t.after(() => transport.close());

  const tools = (await client.listTools()).tools;
  const names = tools.map(({ name }) => name);
  assert.ok(names.includes("cuepool_go"));
  assert.ok(names.includes("cuepool_seek"));
  assert.equal(
    tools.find(({ name }) => name === "cuepool_go")?.annotations
      ?.idempotentHint,
    true,
  );
  assert.equal(
    tools.find(({ name }) => name === "cuepool_go")?.annotations?.openWorldHint,
    true,
  );
  assert.match(
    tools.find(({ name }) => name === "cuepool_go")?.description ?? "",
    /connected outputs/,
  );

  const applied = await client.callTool({
    name: "cuepool_go",
    arguments: { operation_id: "test-go-1" },
  });
  assert.equal(applied.isError, undefined);
  assert.deepEqual(applied.structuredContent, {
    data: {
      id: 7,
      state: "applied",
      message: "GO applied",
      created_at: "2026-08-14T00:00:00.000Z",
      completed_at: "2026-08-14T00:00:00.001Z",
    },
  });

  const rejected = await client.callTool({
    name: "cuepool_pause",
    arguments: { operation_id: "test-pause-1" },
  });
  assert.equal(rejected.isError, true);
  assert.deepEqual(rejected.structuredContent, {
    error: { kind: "rejected", message: "nothing is playing" },
    data: {
      id: 8,
      state: "rejected",
      message: "nothing is playing",
      created_at: "2026-08-14T00:00:00.000Z",
      completed_at: "2026-08-14T00:00:00.001Z",
    },
  });
});

test("API unavailability and disabled control return structured errors", async (t) => {
  const api = await mockApi((request, response) => {
    if (request.method === "POST") {
      json(response, 403, {
        error: "control_disabled",
        message: "set CUEPOOL_API_CONTROL_TOKEN to enable commands",
      });
      return;
    }
    assert.equal(request.headers.authorization, undefined);
    json(response, 503, {
      error: "unavailable",
      message: "CuePool show-control loop is not ready",
    });
  });
  t.after(() => api.close());

  const { client, transport } = await mcpClient(api.url, "configured-token");
  t.after(() => client.close());
  t.after(() => transport.close());

  const health = await client.callTool({
    name: "cuepool_health",
    arguments: {},
  });
  assert.equal(health.isError, true);
  assert.deepEqual(health.structuredContent, {
    error: {
      kind: "http",
      message: "CuePool show-control loop is not ready",
      status: 503,
      code: "unavailable",
    },
    data: {
      error: "unavailable",
      message: "CuePool show-control loop is not ready",
    },
  });

  const go = await client.callTool({
    name: "cuepool_go",
    arguments: { operation_id: "test-go-1" },
  });
  assert.equal(go.isError, true);
  assert.deepEqual(go.structuredContent, {
    error: {
      kind: "http",
      message: "set CUEPOOL_API_CONTROL_TOKEN to enable commands",
      status: 403,
      code: "control_disabled",
    },
    data: {
      error: "control_disabled",
      message: "set CUEPOOL_API_CONTROL_TOKEN to enable commands",
    },
  });
});

test("retrying an uncertain command reuses the caller operation ID", async (t) => {
  let posts = 0;
  let executions = 0;
  const status = {
    id: 21,
    state: "applied",
    message: "GO applied",
    created_at: "2026-08-14T00:00:00.000Z",
    completed_at: "2026-08-14T00:00:00.001Z",
  };
  const api = await mockApi((request, response) => {
    if (request.method === "POST") {
      assert.equal(request.headers["idempotency-key"], "retry-go-1");
      posts += 1;
      if (posts === 1) {
        executions += 1;
        json(response, 202, {
          ...status,
          state: "pending",
          completed_at: null,
        });
      } else {
        json(response, 202, status);
      }
      return;
    }
    response.destroy();
  });
  t.after(() => api.close());

  const { client, transport } = await mcpClient(api.url, "test-token");
  t.after(() => client.close());
  t.after(() => transport.close());

  const first = await client.callTool({
    name: "cuepool_go",
    arguments: { operation_id: "retry-go-1" },
  });
  assert.equal(first.isError, true);
  assert.deepEqual(first.structuredContent, {
    error: {
      kind: "unavailable",
      message: "CuePool API is unavailable: fetch failed",
    },
    data: {
      command: {
        ...status,
        state: "pending",
        completed_at: null,
      },
    },
  });

  const retried = await client.callTool({
    name: "cuepool_go",
    arguments: { operation_id: "retry-go-1" },
  });
  assert.equal(retried.isError, undefined);
  assert.deepEqual(retried.structuredContent, { data: status });
  assert.equal(posts, 2);
  assert.equal(executions, 1);
});

test("command polling shares one end-to-end timeout", async (t) => {
  const pending = {
    id: 31,
    state: "pending",
    message: null,
    created_at: "2026-08-14T00:00:00.000Z",
    completed_at: null,
  };
  const api = await mockApi((request, response) => {
    if (request.method === "POST") {
      json(response, 202, pending);
    }
    // Deliberately leave the status poll open until the adapter aborts it.
  });
  t.after(() => api.close());

  const { client, transport } = await mcpClient(api.url, "test-token", 200);
  t.after(() => client.close());
  t.after(() => transport.close());

  const started = Date.now();
  const result = await client.callTool({
    name: "cuepool_go",
    arguments: { operation_id: "timeout-go-1" },
  });
  const elapsed = Date.now() - started;
  assert.equal(result.isError, true);
  assert.ok(elapsed < 260, `command took ${elapsed}ms`);
  const failure = result.structuredContent as {
    error: { kind: string };
    data: { command: { id: number; state: string } };
  };
  assert.equal(failure.error.kind, "timeout");
  assert.deepEqual(failure.data.command, pending);
});

test("closing STDIO cancels a hung API request and exits promptly", async (t) => {
  let markStarted!: () => void;
  const started = new Promise<void>((resolve) => {
    markStarted = resolve;
  });
  const api = await mockApi(() => markStarted());
  t.after(() => api.close());

  const child = spawn(
    process.execPath,
    [path.join(import.meta.dirname, "index.js")],
    {
      env: {
        CUEPOOL_API_URL: api.url,
        CUEPOOL_API_TIMEOUT_MS: "300000",
      },
      stdio: ["pipe", "pipe", "pipe"],
    },
  );
  t.after(() => child.kill("SIGTERM"));
  const output = createInterface({ input: child.stdout });
  child.stdin.write(
    `${JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: {
        protocolVersion: "2025-06-18",
        capabilities: {},
        clientInfo: { name: "shutdown-test", version: "0.1.0" },
      },
    })}\n`,
  );
  await within(once(output, "line"), 1_000, "MCP initialization timed out");
  child.stdin.write(
    `${JSON.stringify({
      jsonrpc: "2.0",
      method: "notifications/initialized",
    })}\n`,
  );
  child.stdin.write(
    `${JSON.stringify({
      jsonrpc: "2.0",
      id: 2,
      method: "tools/call",
      params: { name: "cuepool_health", arguments: {} },
    })}\n`,
  );
  await within(started, 1_000, "API request did not start");

  const exited = once(child, "exit");
  child.stdin.end();
  const [code] = await within(
    exited,
    1_000,
    "MCP process did not exit after STDIO closed",
  );
  assert.equal(code, 0);
});

test("cancelling one MCP call aborts its API request", async (t) => {
  let markStarted!: () => void;
  let markClosed!: () => void;
  const started = new Promise<void>((resolve) => {
    markStarted = resolve;
  });
  const closed = new Promise<void>((resolve) => {
    markClosed = resolve;
  });
  const api = await mockApi((_request, response) => {
    response.once("close", markClosed);
    markStarted();
  });
  t.after(() => api.close());

  const { client, transport } = await mcpClient(api.url, undefined, 300_000);
  t.after(() => client.close());
  t.after(() => transport.close());

  const cancellation = new AbortController();
  const call = client.callTool(
    { name: "cuepool_health", arguments: {} },
    { signal: cancellation.signal },
  );
  await within(started, 1_000, "API request did not start");
  cancellation.abort();
  await assert.rejects(call);
  await within(closed, 1_000, "cancelled API request stayed open");
});

async function mcpClient(apiUrl: string, token?: string, timeoutMs?: number) {
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [path.join(import.meta.dirname, "index.js")],
    env: {
      CUEPOOL_API_URL: apiUrl,
      ...(token ? { CUEPOOL_API_TOKEN: token } : {}),
      ...(timeoutMs ? { CUEPOOL_API_TIMEOUT_MS: String(timeoutMs) } : {}),
    },
    stderr: "pipe",
  });
  const client = new Client({ name: "cuepool-mcp-test", version: "0.1.0" });
  await client.connect(transport);
  return { client, transport };
}

async function mockApi(
  handler: (
    request: IncomingMessage,
    response: ServerResponse,
  ) => void | Promise<void>,
): Promise<{ close: () => void; url: string }> {
  const server = createServer(
    (request, response) => void handler(request, response),
  );
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  assert.ok(address && typeof address === "object");
  return {
    close: () => server.close(),
    url: `http://127.0.0.1:${address.port}`,
  };
}

function json(response: ServerResponse, status: number, value: unknown): void {
  response.writeHead(status, { "Content-Type": "application/json" });
  response.end(JSON.stringify(value));
}

async function body(request: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of request) {
    chunks.push(Buffer.from(chunk));
  }
  return Buffer.concat(chunks).toString("utf8");
}

async function within<T>(
  promise: Promise<T>,
  timeoutMs: number,
  message: string,
): Promise<T> {
  let timeout: NodeJS.Timeout | undefined;
  try {
    return await Promise.race([
      promise,
      new Promise<never>((_resolve, reject) => {
        timeout = setTimeout(() => reject(new Error(message)), timeoutMs);
      }),
    ]);
  } finally {
    if (timeout) {
      clearTimeout(timeout);
    }
  }
}
