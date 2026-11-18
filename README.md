# java-analyzer

**A Java language server written in Rust. No JVM inside.**

Parsing, name resolution and type inference are implemented from scratch. The JDK and dependency
jars are read as data and indexed; build tools run only to import a Gradle project or to fetch
artifacts missing from the local Maven repository.

- **Silent on valid code.** The full JDK 25 source tree — 15,227 files, 5.15 million lines — and
  seven real-world Maven and Gradle projects analyze with zero error-level diagnostics.
- **Fast where it matters.** Completion in 1.2 ms at p95, hover and go to definition below 0.05 ms;
  an edit re-checks only the method bodies it affects.
- **Measured against javac.** A differential harness mutates a compiler corpus 1,130 ways: 97% of
  javac's errors are caught, and nothing javac accepts is flagged.
- **Beyond the protocol checklist.** Refactorings and quick fixes, flow and nullness analysis,
  Lombok and Spring, decompiled library sources, and a JDWP debugger.

> **Status: early development.** See [known limitations](docs/limitations.md).

## At a glance

| | |
|---|---|
| **Language** | Java up to 25, plus primitive types in patterns (preview); per-module language level |
| **Projects** | Maven, Gradle (Groovy and Kotlin DSL, composite builds), `java-analyzer.toml`; JDK 8 and 9+ |
| **Navigation** | Definition, references, implementations, call and type hierarchies, workspace symbols with CamelCase matching, Javadoc links |
| **Editing** | Context-aware completion with auto-import and postfix templates, rename, formatting from `.editorconfig`, inlay hints, code lens |
| **Diagnostics** | Type checking, flow analysis, `unchecked` warnings, nullness analysis across the common annotation libraries, dozens of inspections with quick fixes |
| **Refactoring** | Extract, inline, change signature, move class and package, introduce parameter, convert to record / `switch` / lambda, and more |
| **Frameworks** | Lombok member synthesis, Spring bean injection and configuration keys, MVC / JAX-RS endpoints linked to Feign clients |
| **Libraries** | Sources from sources jars and `src.zip`, downloaded on demand; decompiled stubs otherwise |
| **Debugging** | `java-analyzer dap`: breakpoints, stepping, expression evaluation with method calls, hot swap |

The full list is in [docs/features.md](docs/features.md).

## Quick start

```sh
cargo build --release    # requires Rust 1.85+; output: target/release/java-analyzer
```

Put `java-analyzer` on your `PATH` with a JDK installed, then set up your editor — VS Code
([extension](editors/code)), Neovim, Helix or Zed — as described in
[docs/editors.md](docs/editors.md).

## Documentation

| | |
|---|---|
| [Features](docs/features.md) | Everything the server does, by area |
| [Editors](docs/editors.md) | Editor setup, debugger, extension requests |
| [Configuration](docs/configuration.md) | Settings, `.editorconfig` keys, language levels |
| [Measurements](docs/measurements.md) | Performance on the JDK and application projects, evaluation results |
| [Limitations](docs/limitations.md) | What is not supported, and why |
| [Architecture](docs/architecture.md) | Crate layering, incremental computation, inference |
| [Evals](evals/README.md) | Scenario suite and javac differential testing |

## Development

```sh
cargo test --workspace         # unit tests do not need a JDK
cargo xtask dist               # build and collect into dist/
cargo xtask codegen            # regenerate AST nodes after editing crates/syntax/ast.def
cargo bench -p syntax          # parsing and incremental reparsing benchmarks
cargo +nightly fuzz run parse  # fuzzing (cargo-fuzz; there is also a reparse target)
java-analyzer analysis-stats <project dir>   # import and analyze a whole project
```

## License

MIT.
