#!/usr/bin/env node

import { isIP } from "node:net";

import { McpServer, type CallToolResult } from "@modelcontextprotocol/server";
import { serveStdio } from "@modelcontextprotocol/server/stdio";
import * as z from "zod/v4";

const DEFAULT_API_URL = "http://127.0.0.1:7133";
const DEFAULT_TIMEOUT_MS = 10_000;
const MAX_API_RESPONSE_BYTES = 24 * 1024 * 1024;
const MAX_ERROR_DATA_BYTES = 1_000_000;
const MAX_ERROR_MESSAGE_BYTES = 16 * 1024;
const MAX_INLINE_TEXT_BYTES = 64_000;
const MAX_LOG_OUTPUT_BYTES = 2_000_000;
const MAX_MCP_OUTPUT_BYTES = 8_000_000;
const POLL_INTERVAL_MS = 75;

type JsonObject = Record<string, unknown>;

type CommandStatus = {
  id: number;
  state: "pending" | "applied" | "rejected";
  message: string | null;
  created_at: string;
  completed_at: string | null;
};

type ErrorDetails = {
  kind: string;
  message: string;
  status?: number;
  code?: string;
};

class CuePoolError extends Error {
  constructor(
    readonly details: ErrorDetails,
    readonly data?: unknown,
  ) {
    super(details.message);
  }
}

class CuePoolClient {
  constructor(
    private readonly baseUrl: URL,
    private readonly token: string | undefined,
    private readonly timeoutMs: number,
    private readonly lifetime: AbortSignal,
  ) {}

  read(route: string, callSignal: AbortSignal): Promise<unknown> {
    return this.request(route, {}, this.timeoutMs, callSignal);
  }

  async command(
    command: JsonObject,
    idempotencyKey: string,
    callSignal: AbortSignal,
  ): Promise<CommandStatus> {
    const deadline = Date.now() + this.timeoutMs;
    const status = commandStatus(
      await this.request(
        "/v1/commands",
        {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            "Idempotency-Key": idempotencyKey,
          },
          body: JSON.stringify(command),
        },
        this.timeoutMs,
        callSignal,
      ),
    );
    let current = status;

    while (current.state === "pending") {
      const beforeSleep = deadline - Date.now();
      if (beforeSleep <= 0) {
        throw commandTimeout(current, this.timeoutMs);
      }
      await new Promise((resolve) =>
        setTimeout(resolve, Math.min(POLL_INTERVAL_MS, beforeSleep)),
      );
      const remaining = deadline - Date.now();
      if (remaining <= 0) {
        throw commandTimeout(current, this.timeoutMs);
      }
      try {
        current = commandStatus(
          await this.request(
            `/v1/commands/${current.id}`,
            {},
            remaining,
            callSignal,
          ),
        );
      } catch (error) {
        if (error instanceof CuePoolError) {
          throw new CuePoolError(error.details, {
            command: current,
            ...(error.data === undefined ? {} : { response: error.data }),
          });
        }
        throw error;
      }
    }

    return current;
  }

  private async request(
    route: string,
    init: RequestInit = {},
    timeoutMs = this.timeoutMs,
    callSignal?: AbortSignal,
  ): Promise<unknown> {
    const headers = new Headers(init.headers);
    if (this.token && init.method === "POST") {
      headers.set("Authorization", `Bearer ${this.token}`);
    }

    let response: Response;
    let text: string;
    const requestAbort = new AbortController();
    let timedOut = false;
    const timeout = setTimeout(() => {
      timedOut = true;
      requestAbort.abort();
    }, timeoutMs);
    const abortForShutdown = () => requestAbort.abort();
    const abortForCall = () => requestAbort.abort();
    if (this.lifetime.aborted) {
      abortForShutdown();
    } else {
      this.lifetime.addEventListener("abort", abortForShutdown, { once: true });
    }
    if (callSignal?.aborted) {
      abortForCall();
    } else {
      callSignal?.addEventListener("abort", abortForCall, { once: true });
    }
    try {
      response = await fetch(new URL(route, this.baseUrl), {
        ...init,
        headers,
        redirect: "error",
        signal: requestAbort.signal,
      });
      text = await readResponseBody(response);
    } catch (error) {
      if (error instanceof CuePoolError) {
        throw error;
      }
      if (this.lifetime.aborted) {
        throw new CuePoolError({
          kind: "cancelled",
          message: "MCP transport closed",
        });
      }
      if (callSignal?.aborted) {
        throw new CuePoolError({
          kind: "cancelled",
          message: "MCP tool call cancelled",
        });
      }
      throw new CuePoolError({
        kind: timedOut ? "timeout" : "unavailable",
        message: timedOut
          ? `CuePool API request timed out after ${timeoutMs}ms`
          : `CuePool API is unavailable: ${errorMessage(error)}`,
      });
    } finally {
      clearTimeout(timeout);
      this.lifetime.removeEventListener("abort", abortForShutdown);
      callSignal?.removeEventListener("abort", abortForCall);
    }

    let data: unknown = null;
    if (text) {
      try {
        data = JSON.parse(text);
      } catch {
        throw new CuePoolError({
          kind: "invalid_response",
          status: response.status,
          message: `CuePool API returned non-JSON data (${response.status})`,
        });
      }
    }

    if (!response.ok) {
      const body = isObject(data) ? data : {};
      const code =
        typeof body.error === "string"
          ? boundedText(body.error, 256)
          : undefined;
      const message =
        typeof body.message === "string"
          ? boundedText(body.message, MAX_ERROR_MESSAGE_BYTES)
          : `CuePool API returned HTTP ${response.status}`;
      throw new CuePoolError(
        {
          kind: "http",
          status: response.status,
          ...(code ? { code } : {}),
          message,
        },
        data,
      );
    }

    return data;
  }
}

function createServer(
  client: CuePoolClient,
  controlEnabled: boolean,
): McpServer {
  const server = new McpServer({
    name: "cuepool",
    version: "0.1.0",
  });

  registerReadTool(
    server,
    client,
    "cuepool_health",
    "Check CuePool readiness and whether control is enabled.",
    "/v1/health",
  );
  registerReadTool(
    server,
    client,
    "cuepool_project",
    "Inspect the currently loaded CuePool project.",
    "/v1/project",
  );
  registerReadTool(
    server,
    client,
    "cuepool_cues",
    "List cues and the current selection.",
    "/v1/cues",
  );
  registerReadTool(
    server,
    client,
    "cuepool_active_cues",
    "Inspect cues that are currently active.",
    "/v1/cues/active",
  );
  registerReadTool(
    server,
    client,
    "cuepool_diagnostics",
    "Inspect CuePool's current runtime diagnostics.",
    "/v1/status",
  );
  server.registerTool(
    "cuepool_logs",
    {
      description: "Read CuePool logs after an optional cursor.",
      inputSchema: z.object({
        after: z
          .number()
          .int()
          .nonnegative()
          .optional()
          .describe("Return entries after this log cursor."),
        limit: z
          .number()
          .int()
          .min(1)
          .max(100)
          .default(100)
          .describe("Maximum log entries to return, from 1 to 100."),
      }),
      annotations: { readOnlyHint: true, openWorldHint: false },
    },
    ({ after, limit }, ctx) =>
      run(
        async () =>
          boundedLogs(
            await client.read(
              `/v1/logs?limit=${limit}${after === undefined ? "" : `&after=${after}`}`,
              ctx.mcpReq.signal,
            ),
            limit,
          ),
        (logs) =>
          `Returned ${logs.entries.length} log entries; next cursor ${logs.next_cursor}${logs.truncated ? " (more available)" : ""}.`,
      ),
  );

  if (controlEnabled) {
    registerControlTool(
      server,
      client,
      "cuepool_select_cue",
      "Select the cue that the next GO will fire.",
      z.object({
        operation_id: operationIdSchema,
        qid: z
          .string()
          .trim()
          .min(1)
          .max(64)
          .regex(/^-?\d+(?:\.\d+)?$/)
          .describe("Decimal Cue QID to select."),
      }),
      ({ qid }) => ({ command: "select_cue", qid }),
    );
    registerControlTool(
      server,
      client,
      "cuepool_go",
      "Fire the selected CuePool cue.",
      z.object({ operation_id: operationIdSchema }),
      () => ({ command: "go" }),
    );
    registerControlTool(
      server,
      client,
      "cuepool_stop",
      "Stop all active CuePool playback.",
      z.object({ operation_id: operationIdSchema }),
      () => ({ command: "stop" }),
    );
    registerControlTool(
      server,
      client,
      "cuepool_pause",
      "Pause active CuePool playback.",
      z.object({ operation_id: operationIdSchema }),
      () => ({ command: "pause" }),
    );
    registerControlTool(
      server,
      client,
      "cuepool_resume",
      "Resume paused CuePool playback.",
      z.object({ operation_id: operationIdSchema }),
      () => ({ command: "resume" }),
    );
    registerControlTool(
      server,
      client,
      "cuepool_preload",
      "Preload the selected CuePool cue.",
      z.object({ operation_id: operationIdSchema }),
      () => ({ command: "preload" }),
    );
    registerControlTool(
      server,
      client,
      "cuepool_seek",
      "Seek an active cue instance.",
      z.object({
        operation_id: operationIdSchema,
        instance_id: z
          .number()
          .int()
          .nonnegative()
          .max(Number.MAX_SAFE_INTEGER),
        seconds: z.number().nonnegative().finite(),
      }),
      ({ instance_id, seconds }) => ({ command: "seek", instance_id, seconds }),
    );
    registerControlTool(
      server,
      client,
      "cuepool_shutdown",
      "Shut down this CuePool profile when playback is stopped and the project has no unsaved changes.",
      z.object({ operation_id: operationIdSchema }),
      () => ({ command: "shutdown" }),
    );
  }

  return server;
}

function registerReadTool(
  server: McpServer,
  client: CuePoolClient,
  name: string,
  description: string,
  route: string,
): void {
  server.registerTool(
    name,
    {
      description,
      inputSchema: z.object({}),
      annotations: { readOnlyHint: true, openWorldHint: false },
    },
    (_input, ctx) => run(() => client.read(route, ctx.mcpReq.signal)),
  );
}

const operationIdSchema = z
  .string()
  .min(1)
  .max(128)
  .regex(/^[A-Za-z0-9._:-]+$/)
  .describe(
    "Unique ID for this action. Reuse the same value when retrying an uncertain result.",
  );

function registerControlTool<
  Input extends JsonObject & { operation_id: string },
>(
  server: McpServer,
  client: CuePoolClient,
  name: string,
  description: string,
  inputSchema: z.ZodType<Input>,
  command: (input: Input) => JsonObject,
): void {
  server.registerTool(
    name,
    {
      description: `${description} Mutates live show-control state and may affect connected outputs.`,
      inputSchema,
      annotations: {
        readOnlyHint: false,
        destructiveHint: true,
        idempotentHint: true,
        openWorldHint: true,
      },
    },
    async (input, ctx) => {
      try {
        const status = await client.command(
          command(input),
          input.operation_id,
          ctx.mcpReq.signal,
        );
        if (status.state === "rejected") {
          return failure(
            new CuePoolError(
              {
                kind: "rejected",
                message:
                  status.message ?? `CuePool rejected command ${status.id}`,
              },
              status,
            ),
          );
        }
        return success(status);
      } catch (error) {
        return failure(error);
      }
    },
  );
}

async function run<T>(
  operation: () => Promise<T>,
  renderText?: (data: T) => string,
): Promise<CallToolResult> {
  try {
    const data = await operation();
    return success(data, renderText?.(data));
  } catch (error) {
    return failure(error);
  }
}

function success(data: unknown, text?: string): CallToolResult {
  const compact = JSON.stringify(data);
  if (compact === undefined) {
    return failure(
      new CuePoolError({
        kind: "invalid_response",
        message: "CuePool returned an unserializable response",
      }),
    );
  }
  const bytes = Buffer.byteLength(compact);
  if (bytes > MAX_MCP_OUTPUT_BYTES) {
    return failure(
      new CuePoolError({
        kind: "response_too_large",
        message: `CuePool response exceeds the ${MAX_MCP_OUTPUT_BYTES}-byte MCP output limit`,
      }),
    );
  }
  return {
    content: [
      {
        type: "text",
        text:
          text ??
          (bytes <= MAX_INLINE_TEXT_BYTES
            ? JSON.stringify(data, null, 2)
            : `${responseSummary(data)} See structured content for the full result.`),
      },
    ],
    structuredContent: { data },
  };
}

function failure(error: unknown): CallToolResult {
  const cuePoolError =
    error instanceof CuePoolError
      ? error
      : new CuePoolError({ kind: "internal", message: errorMessage(error) });
  const details = {
    ...cuePoolError.details,
    message: boundedText(cuePoolError.details.message, MAX_ERROR_MESSAGE_BYTES),
  };
  if (details.code) {
    details.code = boundedText(details.code, 256);
  }
  const structuredContent: JsonObject = { error: details };
  if (
    cuePoolError.data !== undefined &&
    Buffer.byteLength(JSON.stringify(cuePoolError.data)) <= MAX_ERROR_DATA_BYTES
  ) {
    structuredContent.data = cuePoolError.data;
  } else if (cuePoolError.data !== undefined) {
    structuredContent.data_omitted = true;
  }
  return {
    content: [
      { type: "text", text: JSON.stringify(structuredContent, null, 2) },
    ],
    structuredContent,
    isError: true,
  };
}

function commandStatus(value: unknown): CommandStatus {
  if (
    !isObject(value) ||
    typeof value.id !== "number" ||
    !["pending", "applied", "rejected"].includes(String(value.state)) ||
    !(typeof value.message === "string" || value.message === null) ||
    typeof value.created_at !== "string" ||
    !(typeof value.completed_at === "string" || value.completed_at === null)
  ) {
    throw new CuePoolError(
      {
        kind: "invalid_response",
        message: "CuePool API returned an invalid command status",
      },
      value,
    );
  }
  return value as CommandStatus;
}

function commandTimeout(
  status: CommandStatus,
  timeoutMs: number,
): CuePoolError {
  return new CuePoolError(
    {
      kind: "timeout",
      message: `CuePool command ${status.id} did not complete within ${timeoutMs}ms`,
    },
    status,
  );
}

function boundedLogs(
  value: unknown,
  limit: number,
): JsonObject & {
  entries: unknown[];
  next_cursor: number;
  truncated: boolean;
} {
  if (!isObject(value) || !Array.isArray(value.entries)) {
    throw new CuePoolError(
      {
        kind: "invalid_response",
        message: "CuePool API returned invalid logs",
      },
      value,
    );
  }

  const entries: unknown[] = [];
  let bytes = 0;
  for (const entry of value.entries) {
    if (entries.length >= limit) {
      break;
    }
    const entryBytes = Buffer.byteLength(JSON.stringify(entry));
    if (bytes + entryBytes > MAX_LOG_OUTPUT_BYTES) {
      if (entries.length === 0) {
        throw new CuePoolError({
          kind: "invalid_response",
          message: "CuePool log entry exceeds the MCP output limit",
        });
      }
      break;
    }
    entries.push(entry);
    bytes += entryBytes;
  }

  const last = entries.at(-1);
  const lastCursor =
    isObject(last) && typeof last.cursor === "number" ? last.cursor : undefined;
  const apiCursor =
    typeof value.next_cursor === "number" ? value.next_cursor : 0;
  return {
    ...value,
    entries,
    next_cursor: lastCursor ?? apiCursor,
    truncated:
      value.truncated === true || entries.length < value.entries.length,
  };
}

async function readResponseBody(response: Response): Promise<string> {
  const contentLength = Number(response.headers.get("content-length"));
  if (
    Number.isFinite(contentLength) &&
    contentLength > MAX_API_RESPONSE_BYTES
  ) {
    await response.body?.cancel();
    throw new CuePoolError({
      kind: "response_too_large",
      status: response.status,
      message: `CuePool API response exceeds ${MAX_API_RESPONSE_BYTES} bytes`,
    });
  }
  if (!response.body) {
    return "";
  }

  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let bytes = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) {
      break;
    }
    bytes += value.byteLength;
    if (bytes > MAX_API_RESPONSE_BYTES) {
      await reader.cancel();
      throw new CuePoolError({
        kind: "response_too_large",
        status: response.status,
        message: `CuePool API response exceeds ${MAX_API_RESPONSE_BYTES} bytes`,
      });
    }
    chunks.push(value);
  }
  return Buffer.concat(chunks, bytes).toString("utf8");
}

function responseSummary(data: unknown): string {
  if (Array.isArray(data)) {
    return `CuePool returned ${data.length} items.`;
  }
  return "CuePool returned a large response.";
}

function boundedText(value: string, maxBytes: number): string {
  const encoded = Buffer.from(value);
  if (encoded.byteLength <= maxBytes) {
    return value;
  }
  return `${encoded.subarray(0, maxBytes).toString("utf8").replace(/�$/, "")}… [truncated]`;
}

function loadConfig(lifetime: AbortSignal): {
  client: CuePoolClient;
  controlEnabled: boolean;
} {
  const baseUrl = new URL(
    process.env.CUEPOOL_API_URL?.trim() || DEFAULT_API_URL,
  );
  if (
    !["http:", "https:"].includes(baseUrl.protocol) ||
    baseUrl.username ||
    baseUrl.password
  ) {
    throw new Error(
      "CUEPOOL_API_URL must be an HTTP(S) URL without embedded credentials",
    );
  }
  if (baseUrl.protocol === "http:" && !isLoopbackHost(baseUrl.hostname)) {
    throw new Error(
      "CUEPOOL_API_URL must use HTTPS unless it points to a loopback address",
    );
  }
  if (baseUrl.pathname !== "/" || baseUrl.search || baseUrl.hash) {
    throw new Error(
      "CUEPOOL_API_URL must be an origin without a path, query, or fragment",
    );
  }
  const token = process.env.CUEPOOL_API_TOKEN?.trim() || undefined;
  const timeoutText = process.env.CUEPOOL_API_TIMEOUT_MS?.trim();
  const timeoutMs =
    timeoutText === undefined ? DEFAULT_TIMEOUT_MS : Number(timeoutText);
  if (
    !Number.isSafeInteger(timeoutMs) ||
    timeoutMs < 100 ||
    timeoutMs > 300_000
  ) {
    throw new Error(
      "CUEPOOL_API_TIMEOUT_MS must be an integer from 100 to 300000",
    );
  }
  return {
    client: new CuePoolClient(baseUrl, token, timeoutMs, lifetime),
    controlEnabled: token !== undefined,
  };
}

function isObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isLoopbackHost(hostname: string): boolean {
  const host = hostname.replace(/^\[|\]$/g, "").toLowerCase();
  if (host === "localhost" || host === "::1") {
    return true;
  }
  return isIP(host) === 4 && host.split(".")[0] === "127";
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

try {
  const lifetime = new AbortController();
  const abortForShutdown = () => lifetime.abort();
  const { client, controlEnabled } = loadConfig(lifetime.signal);
  process.stdin.once("end", abortForShutdown);
  process.stdin.once("close", abortForShutdown);
  serveStdio(() => createServer(client, controlEnabled), {
    onerror: (error) => console.error(`CuePool MCP error: ${error.message}`),
  });
} catch (error) {
  console.error(`CuePool MCP failed to start: ${errorMessage(error)}`);
  process.exitCode = 1;
}
