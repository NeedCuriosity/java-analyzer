<div align="center">

# java-analyzer

**A Java language server written in Rust. No JVM inside.**

[![CI](https://github.com/l0ng-ai/java-analyzer/actions/workflows/ci.yml/badge.svg)](https://github.com/l0ng-ai/java-analyzer/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/l0ng-ai/java-analyzer)](https://github.com/l0ng-ai/java-analyzer/releases/latest)
![Java](https://img.shields.io/badge/Java-8%20→%2025-orange)

<br>

|  **5.15 M**  |  **0**  |  **1.2 ms**  |  **97 %**  |
|:---:|:---:|:---:|:---:|
| lines of JDK source analyzed | false positives on them | completion, p95 | of javac's errors caught |

</div>

<br>

Parser, name resolution and type inference, rebuilt from scratch as an incremental Rust program.
Your JDK and dependencies are indexed as data, never run — and a diagnostic is reported only when it
can be decided exactly.

## Install

```sh
curl -L https://github.com/l0ng-ai/java-analyzer/releases/latest/download/java-analyzer-linux-x86_64.gz \
  | gunzip > ~/.local/bin/java-analyzer && chmod +x ~/.local/bin/java-analyzer
```

Other platforms are on the [release page](https://github.com/l0ng-ai/java-analyzer/releases/latest).
A JDK must be installed. Then connect your editor: [VS Code](editors/code), or
[Neovim, Helix and Zed](docs/editors.md).

## Documentation

[Features](docs/features.md) · [Configuration](docs/configuration.md) · [Measurements](docs/measurements.md) · [Limitations](docs/limitations.md) · [Architecture](docs/architecture.md) · [Contributing](CONTRIBUTING.md)

## License

MIT
