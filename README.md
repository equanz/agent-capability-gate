# MCP Capability Boundary

You can tell a coding agent “use only this namespace” or “work only in this project.” But a broad CLI or MCP tool still lets it choose every namespace, project, and option. Direct CLI access may also require giving the agent a credential it can read and reuse. Prompts and command rules do not turn either problem into a mechanical boundary.

MCP Capability Boundary turns one reviewed operation into the only interface an agent receives.

- Expose a narrow slice of an existing CLI or MCP server as a typed MCP tool.
- Fix the operation, scope, arguments, environment, and credential authority under administrator control.
- Accept only a closed input schema and resolve it to exact CLI or MCP arguments—never a shell command string.

Read the [user guide](docs/usage.md) for the threat model, deployment boundary, configuration model, and constraints.

```mermaid
flowchart LR
    A["Coding agent"] -->|"typed MCP call"| B["MCP Capability Boundary<br/>admin-owned config"]
    B -->|"fixed argv / MCP arguments"| T["CLI or upstream MCP<br/>credential outside agent"]
```

## Install

Rust 1.98.1 is pinned for this repository. From a source checkout:

```sh
cargo install --locked --offline --path crates/boundary --bin mcp-boundary
```

## CLI example

Turn `git` into one capability: read a bounded number of recent commits from one fixed repository. Create `/absolute/path/git-history.yaml` and adjust the administrator-owned repository path:

```yaml
version: 1

server:
  name: git-history
  transport: { kind: stdio }

targets:
  git:
    kind: cli
    executable: /usr/bin/git
    cwd: /srv/example-app # The agent cannot select another repository.
    limits:
      timeout_ms: 5000
      stdout_bytes: 65536
      stderr_bytes: 4096

tools:
  recent_commits:
    description: Read recent commits from the configured repository
    input_schema:
      type: object
      additionalProperties: false # Undeclared inputs are rejected.
      properties:
        limit:
          type: integer
          minimum: 1
          maximum: 20 # The only caller-controlled value is bounded.
      required: [limit]
    invoke:
      target: git
      cli:
        argv:
          - literal: log # The operation and argv shape are fixed.
          - literal: --oneline
          - literal: --no-decorate
          - literal: --max-count
          - input: /limit
    output: { kind: text }
```

Validate the configuration and inspect the public catalog without running the target:

```sh
mcp-boundary check --config /absolute/path/git-history.yaml
mcp-boundary tools --config /absolute/path/git-history.yaml --format json
```

When authoring a boundary for a real target, use the included [capability-config authoring skill](skills/author-capability-gate-config/SKILL.md). It guides the authority, target-semantics, deployment, and adversarial-input review that YAML validation cannot prove.

## Upstream MCP example

The MCP reference Time server itself needs no credential, external network, or writable data source. The quickest launch path is `uvx mcp-server-time`; set `command` to the absolute output of `command -v uvx`:

```yaml
version: 1

server:
  name: utc-clock
  transport: { kind: stdio }

targets:
  time:
    kind: mcp
    transport:
      kind: stdio
      command: /absolute/path/to/uvx
      args: [mcp-server-time] # Launch Time, not a caller-selected server.
      cwd: /
    limits:
      timeout_ms: 5000
      output_bytes: 65536
      stderr_bytes: 4096

tools:
  current_time_utc:
    description: Get the current time in UTC
    input_schema:
      type: object
      additionalProperties: false
      properties: {}
      required: []
    invoke:
      target: time
      mcp:
        tool: get_current_time # Other upstream tools are not published.
        arguments:
          timezone: { literal: UTC } # The caller cannot select another timezone.
    output: { kind: text }
```

The upstream server may provide other tools and timezone inputs; they are not published by this configuration. `uvx` may download or update the package on launch. For a durable boundary, install and pin it first, then replace `command` with the absolute `mcp-server-time` executable and use `args: []`.

## Run

Validate the selected configuration, then register this command and argument vector as a STDIO MCP server in the agent or MCP client:

```text
mcp-boundary serve --config /absolute/path/boundary.yaml
```

The executable, configuration, target programs, MCP registration, and credential sources must be outside the coding agent's write authority. Credentials must also be outside its read authority. See the [user guide](docs/usage.md) before using privileged targets.

## Documentation

- [User guide](docs/usage.md): motivation, configuration, deployment, and current constraints
- [System design](docs/design.md): normative behavior and trust boundaries
- [Observability](docs/observability-design.md): opt-in debug events and information boundaries
- [Verification](docs/verification-design.md): acceptance scenarios and test strategy
- [Capability-config authoring skill](skills/author-capability-gate-config/SKILL.md): review a target and its deployment before publishing it to an agent

## License

[MIT License](LICENSE)
