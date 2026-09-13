// Lists the tools an MCP stdio server offers, as JSON: used by
// scripts/parity-tools.sh with the TypeScript and the Rust shim in turn.
// Usage: tsx scripts/tools-list.ts <command> [args...]
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
const [command, ...args] = process.argv.slice(2);
const client = new Client({ name: "tools-list", version: "0" });
await client.connect(new StdioClientTransport({ command: command!, args }));
const tools = await client.listTools();
console.log(JSON.stringify(tools.tools, null, 2));
await client.close();
