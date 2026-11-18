# java-analyzer for VS Code

Java language support (completion, navigation, diagnostics, refactorings, formatting, Spring and
Lombok support, debugging) through the [java-analyzer](https://github.com/l0ng-ai/java-analyzer)
language server.

The extension does not bundle the server: put `java-analyzer` on your `PATH`, or set
`java-analyzer.server.path`. A JDK must be installed (see the main README).

Disable other Java language extensions (such as *Language Support for Java by Red Hat*) in the
workspace, or both servers will answer every request.

## Features

- Everything the server offers through LSP on `.java` files, and on Spring Boot configuration
  files (`application*.properties` / `.yml` / `.yaml`, `bootstrap*`).
- Clicking a reference or implementation count above a declaration opens a peek view listing them.
- Settings under `java-analyzer.*` are passed to the server and applied without a restart (see
  [configuration](https://github.com/l0ng-ai/java-analyzer/blob/main/docs/configuration.md)).
- Debugging through `java-analyzer dap`: debug type `java-analyzer` with `launch` (`mainClass`,
  `projectRoot`, `classPaths`, `vmArgs`, `args`, …) and `attach` (`hostName`, `port`). The debugger
  does not build the project; compile it first.
- Hot swap: after recompiling, **java-analyzer: Hot Swap Changed Classes** (also on the debug toolbar)
  replaces the changed classes in the running JVM. Only method bodies can change; adding or removing
  methods or fields is reported as a failure and needs a restart.
- Commands: **java-analyzer: Restart Server**, **java-analyzer: Show Output**,
  **java-analyzer: Hot Swap Changed Classes**.

The server log goes to the *java-analyzer* output channel; set
`"java-analyzer.server.extraEnv": { "JA_LOG": "info" }` for more detail, and
`java-analyzer.trace.server` to trace the LSP messages.

## Building

```sh
npm ci
npm run build       # bundle into out/main.js
npm run typecheck
npm run package     # produce a .vsix, install with `code --install-extension java-analyzer-*.vsix`
```
