import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";
import {
  DidChangeConfigurationNotification,
  Executable,
  LanguageClient,
  LanguageClientOptions,
  Location as LspLocation,
  Position as LspPosition,
  ServerOptions,
} from "vscode-languageclient/node";

const SECTION = "java-analyzer";
/** Client-side command the server attaches to code lenses: `[uri, position, locations]`. */
const SHOW_REFERENCES = "java-analyzer.showReferences";

let client: LanguageClient | undefined;
let outputChannel: vscode.OutputChannel;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  outputChannel = vscode.window.createOutputChannel("java-analyzer");
  context.subscriptions.push(outputChannel);

  context.subscriptions.push(
    vscode.commands.registerCommand("java-analyzer.restartServer", restart),
    vscode.commands.registerCommand("java-analyzer.showOutput", () => outputChannel.show(true)),
    vscode.commands.registerCommand(SHOW_REFERENCES, showReferences),
    vscode.commands.registerCommand("java-analyzer.redefineClasses", redefineClasses),
    vscode.debug.registerDebugAdapterDescriptorFactory(SECTION, {
      createDebugAdapterDescriptor: () =>
        new vscode.DebugAdapterExecutable(serverPath(), ["dap"], { env: serverEnv() }),
    }),
    vscode.workspace.onDidChangeConfiguration(onConfigurationChanged),
  );

  await start();
}

export async function deactivate(): Promise<void> {
  await stop();
}

function config(): vscode.WorkspaceConfiguration {
  return vscode.workspace.getConfiguration(SECTION);
}

/** The executable from `java-analyzer.server.path`, or `java-analyzer` looked up on `PATH`. */
function serverPath(): string {
  const configured = config().get<string>("server.path")?.trim();
  if (!configured) {
    return "java-analyzer";
  }
  if (configured === "~" || configured.startsWith("~/")) {
    return path.join(os.homedir(), configured.slice(1));
  }
  return configured;
}

function serverEnv(): Record<string, string> {
  const extra = config().get<Record<string, string>>("server.extraEnv") ?? {};
  const env: Record<string, string> = {};
  for (const [key, value] of Object.entries(process.env)) {
    if (value !== undefined) {
      env[key] = value;
    }
  }
  return { ...env, ...extra };
}

/**
 * The `java-analyzer.*` settings as the object the server expects (see docs/configuration.md),
 * without the keys that only concern this extension.
 */
function serverSettings(): Record<string, unknown> {
  const settings = JSON.parse(JSON.stringify(config())) as Record<string, unknown>;
  delete settings.server;
  delete settings.trace;
  // Code lenses carry plain JSON arguments; route them through a command that converts them
  // into the objects `editor.action.showReferences` requires.
  const codeLens = (settings.codeLens ?? {}) as Record<string, unknown>;
  if (codeLens.command === undefined || codeLens.command === null) {
    codeLens.command = SHOW_REFERENCES;
  }
  settings.codeLens = codeLens;
  return settings;
}

async function start(): Promise<void> {
  const executable: Executable = {
    command: serverPath(),
    options: { env: serverEnv() },
  };
  const serverOptions: ServerOptions = { run: executable, debug: executable };
  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: "file", language: "java" },
      // Spring Boot configuration files; the server ignores other YAML / properties files
      { scheme: "file", pattern: "**/{application,bootstrap}*.{properties,yml,yaml}" },
    ],
    initializationOptions: serverSettings(),
    outputChannel,
  };
  const created = new LanguageClient(SECTION, "java-analyzer", serverOptions, clientOptions);
  client = created;
  try {
    await created.start();
  } catch (err) {
    client = undefined;
    const message = err instanceof Error ? err.message : String(err);
    const choice = await vscode.window.showErrorMessage(
      `Failed to start java-analyzer (${serverPath()}): ${message}`,
      "Open Settings",
      "Show Output",
    );
    if (choice === "Open Settings") {
      await vscode.commands.executeCommand("workbench.action.openSettings", "java-analyzer.server.path");
    } else if (choice === "Show Output") {
      outputChannel.show(true);
    }
  }
}

async function stop(): Promise<void> {
  const running = client;
  client = undefined;
  if (running) {
    try {
      await running.stop();
    } catch (err) {
      outputChannel.appendLine(`Error while stopping the server: ${err}`);
    }
  }
}

async function restart(): Promise<void> {
  await stop();
  await start();
}

async function onConfigurationChanged(event: vscode.ConfigurationChangeEvent): Promise<void> {
  if (!event.affectsConfiguration(SECTION)) {
    return;
  }
  if (
    event.affectsConfiguration(`${SECTION}.server.path`) ||
    event.affectsConfiguration(`${SECTION}.server.extraEnv`)
  ) {
    await restart();
    return;
  }
  if (client?.isRunning()) {
    await client.sendNotification(DidChangeConfigurationNotification.type, {
      settings: serverSettings(),
    });
  }
}

async function showReferences(uri: string, position: LspPosition, locations: LspLocation[]): Promise<void> {
  const converter = client?.protocol2CodeConverter;
  if (!converter) {
    return;
  }
  await vscode.commands.executeCommand(
    "editor.action.showReferences",
    converter.asUri(uri),
    converter.asPosition(position),
    (locations ?? []).map((location) => converter.asLocation(location)),
  );
}

interface RedefinedClass {
  name: string;
  status: "redefined" | "failed";
  message?: string;
}

/** Hot-swap the classes recompiled since the debug session started (or since the last hot swap). */
async function redefineClasses(): Promise<void> {
  const session = vscode.debug.activeDebugSession;
  if (!session || session.type !== SECTION) {
    void vscode.window.showWarningMessage("No java-analyzer debug session is active.");
    return;
  }
  let result: { classes?: RedefinedClass[]; notLoaded?: number };
  try {
    result = await session.customRequest("redefineClasses");
  } catch (err) {
    void vscode.window.showErrorMessage(`Hot swap failed: ${err instanceof Error ? err.message : String(err)}`);
    return;
  }
  const classes = result.classes ?? [];
  const failed = classes.filter((c) => c.status === "failed");
  const redefined = classes.length - failed.length;
  if (failed.length > 0) {
    const names = failed.map((c) => c.name).join(", ");
    void vscode.window.showWarningMessage(
      `Hot swap failed for ${names}: ${failed[0].message ?? ""}` +
        (redefined > 0 ? ` (${redefined} other class(es) were replaced)` : ""),
    );
  } else if (redefined > 0) {
    void vscode.window.showInformationMessage(`Hot swapped ${redefined} class(es).`);
  } else {
    void vscode.window.showInformationMessage("No loaded class has changed since the last hot swap.");
  }
}
