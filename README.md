<div align="center">

# java-analyzer

**A Java language server written in Rust. No JVM inside.**

[![CI](https://github.com/l0ng-ai/java-analyzer/actions/workflows/ci.yml/badge.svg)](https://github.com/l0ng-ai/java-analyzer/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/l0ng-ai/java-analyzer)](https://github.com/l0ng-ai/java-analyzer/releases/latest)
![Java](https://img.shields.io/badge/Java-8%20→%2025-orange)
![License](https://img.shields.io/badge/license-MIT-blue)

<br>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/project-check-time-dark.svg">
  <img alt="Time from opening a project to every file checked: spring-petclinic 0.42 s, eladmin 0.68 s, RuoYi-Vue 0.53 s, pig 1.07 s, mall 1.23 s, mockito 1.44 s, resilience4j 1.73 s" src="docs/assets/project-check-time-light.svg" width="720">
</picture>

<sub>Open-source Maven and Gradle projects, release build on an 8-core Linux machine.
<a href="docs/measurements.md">How this was measured</a></sub>

</div>

<br>

java-analyzer reimplements the front half of a Java compiler (parser, name resolution, type
inference, flow analysis) as an incremental Rust program, and builds an IDE on top of it. Your JDK
and dependencies are read as data, never run. A diagnostic is reported only when it can be decided
exactly, so an error in the editor is an error `javac` would report too.

## Highlights

- **Fast where it counts.** Completion p95 is 1.2 ms on the JDK sources; after an edit only the
  method body you touched is checked again.
- **No false positives.** Zero error-level diagnostics on all 5.15 million lines of the JDK, while
  catching 97 % of the errors `javac` reports in [differential testing](evals/README.md).
- **Java 8 through 25,** including records, sealed types, pattern matching, module declarations,
  compact source files and current preview features, with the language level set per module.
- **Maven and Gradle** projects import directly from `pom.xml` and the Gradle model, with test roots,
  library scopes and multi-module reactors.
- **Frameworks understood.** Lombok, Spring beans and configuration keys, Spring MVC / JAX-RS
  endpoints and the OpenFeign clients that call them.
- **Libraries without sources** open as stubs with method bodies decompiled from bytecode.
- **A debugger in the same binary.** `java-analyzer dap` speaks JDWP directly: breakpoints,
  expression evaluation, hot swap.

The full list is in [docs/features.md](docs/features.md).

## Install

A JDK must be installed; the server finds it on its own, or set `jdkHome`.

**macOS and Linux**

```sh
os=$(uname -s | sed 's/Darwin/macos/;s/Linux/linux/'); arch=$(uname -m | sed 's/arm64/aarch64/')
mkdir -p ~/.local/bin
curl -fL "https://github.com/l0ng-ai/java-analyzer/releases/latest/download/java-analyzer-$os-$arch.gz" \
  | gunzip > ~/.local/bin/java-analyzer && chmod +x ~/.local/bin/java-analyzer
```

**Windows:** download `java-analyzer-windows-x86_64.zip` (or `aarch64`) from the
[latest release](https://github.com/l0ng-ai/java-analyzer/releases/latest) and put
`java-analyzer.exe` on your `PATH`.

**From source** (Rust 1.85+):

```sh
cargo install --locked --git https://github.com/l0ng-ai/java-analyzer java-analyzer
```

## Editor setup

**VS Code.** Build and install the extension in [`editors/code`](editors/code):

```sh
cd editors/code && npm ci && npm run package
code --install-extension java-analyzer-*.vsix
```

Disable other Java language extensions in the workspace.

**Neovim 0.11+**

```lua
-- ~/.config/nvim/lsp/java_analyzer.lua
return {
  cmd = { "java-analyzer" },
  filetypes = { "java" },
  root_markers = { "settings.gradle", "settings.gradle.kts", "pom.xml", "build.gradle", ".git" },
}
```

```lua
-- init.lua
vim.lsp.enable("java_analyzer")
```

**Helix**

```toml
# ~/.config/helix/languages.toml
[language-server.java-analyzer]
command = "java-analyzer"

[[language]]
name = "java"
language-servers = ["java-analyzer"]
```

Zed, debugger setup and the remaining options are in [docs/editors.md](docs/editors.md).

## Configuration

Settings are passed as `initializationOptions` (or `java-analyzer.*` settings in VS Code); projects
the build files cannot describe can be configured with a `java-analyzer.toml`. See
[docs/configuration.md](docs/configuration.md).

## Documentation

| | |
|---|---|
| [Features](docs/features.md) | Everything the server does, by area |
| [Editors](docs/editors.md) | VS Code, Neovim, Helix, Zed and the debugger |
| [Configuration](docs/configuration.md) | Server options, `java-analyzer.toml`, language levels, code style |
| [Measurements](docs/measurements.md) | Speed and accuracy on the JDK and real projects |
| [Limitations](docs/limitations.md) | What is deliberately not checked, and why |
| [Architecture](docs/architecture.md) | How the analysis is put together |

## Contributing

Bug reports with a snippet that reproduces them are the most useful thing you can send. Any
error-level diagnostic on code that compiles is a bug. For building and testing, see
[CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT
