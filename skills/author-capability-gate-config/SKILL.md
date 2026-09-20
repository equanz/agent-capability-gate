---
name: author-capability-gate-config
description: Create or review an MCP Capability Boundary config when an administrator needs to expose a narrowly defined CLI or upstream MCP capability to an LLM.
---

# Author Capability Gate Config

Turn an administrator's intended authority into an explicit, reviewable capability configuration. This skill helps author configurations for this repository's `mcp-boundary`; it does not certify that a target is safe or infer its authorization semantics.

## Establish the authority first

Before writing YAML, state the capability in one sentence: target operation, permitted subject/scope, fixed identity and credential authority, allowed result, and what the caller cannot select. Split cases whose inputs can change operation class, destructive effect, scope, or credential authority into separate tools.

Make a compact authority table for every input and fixed value. Classify each item as command/subcommand, option name, option value, scope, identity/credential selector, path/URL/config selector, or payload. Ask for a decision when the intended authority is not fixed enough to classify; do not silently choose it.

## Design the deployment boundary

Identify the coding agent and its actual execution surface: local host, remote workspace, container, or hosted runner. Consult the current documentation and effective sandbox or managed policy for that agent. A repository setting is evidence only if the agent and its child processes cannot modify or bypass it.

For the broker executable, capability config, MCP registration or launcher, target executable or upstream server, and credentials, record the concrete location or owner and whether the administrator, broker process, agent, and agent-started child processes can read, write, execute, replace, or invoke it. Check parent directories and symlink targets as well as the file itself.

Require both of these properties:

- the agent and its child processes cannot modify the broker, config, registration, target, or credential source; and
- the agent cannot invoke the target or upstream directly with the broker's identity or credentials, bypassing the published MCP tools.

Placement outside the workspace is not sufficient when the same OS identity can change permissions, replace a parent-directory entry, or invoke the target directly. Prefer administrator-owned installation paths and config directories outside every agent-writable root only when the effective sandbox enforces that separation. On a managed macOS host, `/usr/local/libexec/mcp-boundary/mcp-boundary` and `/Library/Application Support/mcp-boundary/<instance>/config.yaml` are reasonable candidates; on a managed Linux host, use an administrator-owned executable under `/usr/local/libexec` and config under `/etc/mcp-boundary`. Treat these as proposals to verify, not intrinsically safe paths.

If the coding agent lacks an enforceable filesystem and process boundary, place the broker and credentials under a separate OS identity, service, container, VM, or host and expose only the MCP interface. If the available transport or agent integration cannot preserve that separation, report the deployment as blocked instead of presenting a same-user read-only file as a security boundary. Keep secrets out of the capability config where possible; when the config contains a secret, the agent must lack read access as well as write access.

## Evaluate two separate boundaries

The product structurally enforces a fixed absolute executable, fixed cwd and environment, closed schemas, explicit bindings, and no shell command string. A scalar input remains one `argv` element and cannot create a new binding node; a declared `each` binding intentionally emits one element per array item. These properties do **not** establish what the target will do with an accepted argv element or array.

Review the target's documented behavior and, when relevant, its implementation for semantic escape hatches. Treat these as blockers until a narrow safe form is established or the capability is redesigned:

- shells, interpreters, or launchers (`sh -c`, `env`, language `-c`/`-e`, wrappers that evaluate arguments);
- caller-selected subcommands, option names, or unbounded argument positions;
- unconstrained scalar `input` or `each` bindings, especially values beginning with `-`;
- caller-selected paths, URLs, config files, kubeconfigs, endpoints, identities, credential names, or scopes;
- target mini-languages such as templates, expressions, queries, `-exec`, proxy commands, or plugin hooks;
- an upstream MCP tool or CLI implementation that turns data into commands, fetches caller-selected code/configuration, or has broader side effects than the stated capability.

Use source documentation and a bounded local inspection where available. If behavior cannot be established, leave the target unexposed rather than treating structural argv safety as a semantic authorization guarantee.

When a blocker grants arbitrary code execution, caller-selected executable or option structure, credential or endpoint replacement, or another authority broader than the stated capability, do not write or return an installable candidate config merely to demonstrate that structural validation accepts it. Return the authority analysis, the blocking argument shape, adversarial examples, and a narrower redesign. If a fragment is necessary to explain the issue, keep it incomplete and label it as prohibited rather than validating it as a candidate.

## Author the narrow configuration

Use the version-1 configuration interface in `docs/design.md`. Keep executable, cwd, environment, upstream command, upstream tool name, credential source, identity, and authority-defining literals outside caller input. Do not add schema fields merely because the target accepts them.

- Define each `input_schema` as a closed object; make every public property required.
- Prefer fixed `literal` bindings. For a variable, use the narrowest `enum`, numeric range, length, and anchored `pattern` that expresses the intended set.
- Bind each input to one known value position. Do not construct flags, concatenate templates, forward the root object, or make conditional argv shapes.
- Use `each` only for a bounded, non-empty scalar array whose every element has the same reviewed meaning. Otherwise use an enum or separate tools.
- Keep child environment deny-by-default. Inherit only a named, necessary variable; never expose credential selection to input or normal diagnostics.
- Make `title`, `description`, and annotations true for every accepted input, but do not treat annotations as enforcement.

## Validate without invoking a target

Use only the non-mutating management commands against the candidate config:

```sh
mcp-boundary check --config /absolute/path/to/config.yaml
mcp-boundary tools --config /absolute/path/to/config.yaml --format json
```

Confirm that `check` succeeds and that the rendered public catalog contains only the intended tool names, schemas, descriptions, and annotations—never target settings or secret values. These commands validate configuration structure and static paths; they do not run a target or validate its semantics. Do not replace this boundary with a real target invocation. If execution testing is separately authorized, use an isolated fake or demonstrably non-mutating target and report that it is a separate test.

## Attack the proposed interface

For every caller-controlled field, write adversarial scenarios and the expected result before accepting the config. Include, when applicable: unknown property, wrong type, boundary and over-boundary lengths/ranges, empty or oversized arrays, NUL, whitespace and metacharacters, leading dash/option injection, values that resemble a sibling subcommand, traversal/absolute path, URL scheme/host change, identity or config replacement, and target mini-language payloads. Include a scenario proving that rejected input does not invoke the target.

Report the scenarios with their expected schema rejection or the exact reviewed capability they exercise. Do not call a scenario safe merely because shell metacharacters remain one argv element: explain how the target interprets it.

## Deliverables and stopping rule

Return:

1. the authority statement and authority table;
2. the coding-agent-specific deployment table, proposed concrete locations, and evidence that direct invocation and modification are denied;
3. for an acceptable design, the config and non-mutating validation results; for a blocked design, a narrower redesign without an installable unsafe config;
4. the target-semantics evidence or unresolved blocker;
5. adversarial scenarios and expected outcomes; and
6. residual authority: everything the selected target, identity, credentials, fixed literals, accepted input, and deployment identity can still do.

End with the limits of this review. A valid config and passing `check`/`tools` output establish only the product's configuration boundary; target behavior, administrator authority, binary replacement, and output content remain outside that proof.
