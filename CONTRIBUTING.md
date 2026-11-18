# Contributing

Requires Rust 1.85+. The design is described in [docs/architecture.md](docs/architecture.md).

```sh
cargo build --release          # target/release/java-analyzer
cargo test --workspace         # unit tests do not need a JDK; end-to-end tests use JAVA_HOME
cargo xtask dist               # build and collect into dist/
cargo xtask codegen            # regenerate AST nodes after editing crates/syntax/ast.def
cargo bench -p syntax          # parsing and incremental reparsing benchmarks
cargo +nightly fuzz run parse  # fuzzing (cargo-fuzz; there is also a reparse target)
```

Checking against real code:

```sh
java-analyzer parse <file or directory>       # syntax errors and parse timing
java-analyzer analysis-stats <project dir>    # import and analyze a whole project
cargo run --release -p evals --bin ja-eval    # scenario suite and javac differential testing
```

Error-level diagnostics on code that compiles are false positives. Before adding a check, run it over
the JDK sources and keep the count at zero; see [evals/README.md](evals/README.md) for the evaluation
harness and its baseline.
