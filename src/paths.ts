import { homedir } from "node:os";
import { join } from "node:path";

// Approved global state root layout (docs/specs/NAMING.md).
export interface StatePaths {
  root: string;
  eventsLog: string;
  indexDb: string;
  artifactsDir: string;
  workspacesDir: string;
  runtimeDir: string;
  logsDir: string;
  configFile: string;
  socketPath: string;
  pidFile: string;
  lockFile: string;
}

export function statePaths(root: string = join(homedir(), ".taskrunner")): StatePaths {
  const runtimeDir = join(root, "runtime");
  return {
    root,
    eventsLog: join(root, "events.jsonl"),
    indexDb: join(root, "index.db"),
    artifactsDir: join(root, "artifacts"),
    workspacesDir: join(root, "workspaces"),
    runtimeDir,
    logsDir: join(root, "logs"),
    configFile: join(root, "config.toml"),
    socketPath: join(runtimeDir, "daemon.sock"),
    pidFile: join(runtimeDir, "daemon.pid"),
    lockFile: join(runtimeDir, "daemon.lock"),
  };
}
