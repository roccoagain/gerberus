# MCP server

```sh
gerberus mcp
```

serves Gerberus to an AI agent over the [Model Context Protocol](https://modelcontextprotocol.io), on stdin and
stdout. Each tool runs the command of the same name and returns what it prints, so it gives exactly what the
command line would:

| Tool | Command |
|---|---|
| `check` | `gerberus check`, with `show` set to `unknown` or `all` to list more than errors and warnings |
| `doctor` | `gerberus doctor` |
| `voltages` | `gerberus voltages`, with `explain` (net names) and `fault` |
| `signal` | `gerberus signal`, with `show` |
| `loads` | `gerberus loads` |
| `parts` | `gerberus parts` |
| `netlist` | `gerberus netlist` |
| `traces` | `gerberus traces`, with `show` |
| `pinout` | `gerberus pinout` |
| `diff` | `gerberus diff`, with `new` (a second design) or `rev` (a git revision) |
| `part_show` | `gerberus part show`, with `mpn` |

Every tool but `part_show` takes `design`, the path to the schematic. A check that fails with an error is a normal
result, not a tool error; the text says so at the end.

To add it to Claude Code:

```sh
claude mcp add gerberus -- gerberus mcp
```

Other clients take the same command (`gerberus` with the argument `mcp`) in their MCP server settings.
