# CuePool MCP server

This small STDIO server lets MCP clients inspect and control a running CuePool
instance through CuePool's `/v1` automation API. It does not contain any
show-control logic of its own.

Requires Node.js 20 or newer. Build it once:

```sh
cd examples/cuepool/mcp
npm ci
npm run build
```

Then add it to an MCP client's configuration using an absolute path:

```json
{
  "mcpServers": {
    "cuepool": {
      "command": "node",
      "args": [
        "/absolute/path/to/rustjay-engine/examples/cuepool/mcp/dist/index.js"
      ],
      "env": {
        "CUEPOOL_API_URL": "http://127.0.0.1:7133"
      }
    }
  }
}
```

Without a token the server advertises only read tools for health, project,
cues, active cues, diagnostics, and logs. To advertise selection and transport
controls, launch CuePool with `CUEPOOL_API_CONTROL_TOKEN` and pass the same
value to this process as `CUEPOOL_API_TOKEN`:

```json
"env": {
  "CUEPOOL_API_URL": "http://127.0.0.1:7133",
  "CUEPOOL_API_TOKEN": "replace-with-your-token"
}
```

Every control call requires an `operation_id`. Choose a unique value for the
action and reuse it if a result is uncertain and must be retried; CuePool then
returns the original command instead of executing it twice.

`cuepool_shutdown` stops only the CuePool profile serving `CUEPOOL_API_URL`.
CuePool returns the final acknowledgement before exiting and rejects shutdown
while playback is active or the project has unsaved changes.

Control tools mutate live show-control state and may affect connected audio,
video, lighting, or network outputs. MCP clients should treat them as external
side effects and apply their normal confirmation policy.

`CUEPOOL_API_TIMEOUT_MS` optionally changes the 10-second request/command
timeout. CuePool must already be running; this MCP process is a sidecar and
does not launch or host the application. Plain HTTP API URLs are accepted only
for loopback addresses; use HTTPS for a remote tunnel or reverse proxy.
