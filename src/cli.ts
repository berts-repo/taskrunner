#!/usr/bin/env node

const USAGE = `Usage: taskrunner <command>

Commands:
  up      Start the Taskrunner daemon in the foreground.
  down    Stop the running daemon.
  status  Report daemon and capture status.
  mcp     Run the stdio MCP shim (auto-starts the daemon).
`;

async function main(argv: string[]): Promise<number> {
  const command = argv[0];
  switch (command) {
    case "up":
    case "down":
    case "status":
    case "mcp":
      process.stderr.write(`taskrunner ${command}: not implemented yet\n`);
      return 1;
    case undefined:
    case "help":
    case "--help":
    case "-h":
      process.stdout.write(USAGE);
      return command === undefined ? 1 : 0;
    default:
      process.stderr.write(`taskrunner: unknown command '${command}'\n\n${USAGE}`);
      return 1;
  }
}

process.exitCode = await main(process.argv.slice(2));
