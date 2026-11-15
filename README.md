# java-analyzer

A Java language server written in Rust. Parsing, name resolution and type inference are all
implemented from scratch and do not depend on a JVM: the JDK and dependency jars are only read as
data to be indexed, and build tools are invoked only when importing a Gradle project (or when the
local Maven repository is missing artifacts).

> **Status: early development.** All unit tests and in-process end-to-end tests pass, and the whole
> JDK 25 source tree has been analyzed in full (see [Measurements](#measurements)); the Gradle
> import has not been run against a real Gradle yet, see [Known limitations](#known-limitations).

## Features

| Area | What is supported |
|---|---|
| Navigation | Go to definition / declaration / type definition / implementation, find references, document highlights, workspace symbols, call hierarchy, type hierarchy |
| Completion | Members, locals, static members, types (auto-import for types not yet imported), keywords, snippets, postfix templates (`.var` `.if` `.for` `.nn` …), override stubs, Javadoc tags; ranked by expected type, scope distance and in-file usage frequency |
| Information | Hover (signature + Javadoc), signature help, semantic tokens (full / range / delta) |
| Structure | Document symbols, folding, selection ranges |
| Editing | Rename (including override chains, constructors and the matching file); formatting (indentation, spacing within a line, consecutive blank lines) and on-type formatting (indent on Enter, re-layout of the current line after `}` and `;`); inlay hints; code lens (reference and implementation counts) |
| Quick fixes | Import, create method / field / local variable, add `throws` or surround with try/catch, add a cast, add missing enum `switch` cases, initialize a variable, replace string `==` with `equals()` |
| Generation and refactoring | Organize imports; generate constructor / getter / setter / `toString` / `equals` / `hashCode`; implement abstract methods; add `@Override`; extract local variable / method / constant; inline local variable / single-statement method; remove unused parameter; move parameter; pull method up to the superclass; convert anonymous class to lambda; invert `if`; replace `var` with an explicit type |
| Diagnostics | Syntax errors; type checking (unresolved symbols / types / imports, type mismatches, arguments not matching any overload, operator operands, non-boolean conditions, duplicate local variables, assignment to `final`, access to private members, instance members referenced from a static context, unimplemented abstract methods, invalid overrides, non-exhaustive `switch`); flow analysis (unreachable statements, missing `return`, reads of uninitialized variables, unhandled checked exceptions); inspections (null dereference, null checks that are always true / false, unused imports / locals / private members, strings compared with `==`, self assignment / comparison) |
| Frameworks | Member synthesis for common Lombok annotations; Spring MVC / JAX-RS endpoints, searchable by path through workspace symbols (`/api/users`, `GET /orders`) |
| Projects | Maven, Gradle, manual configuration via `java-analyzer.toml`; automatic JDK discovery |
| Libraries | Going to a dependency class opens the source from the sources jar / `src.zip`, or a generated signature stub when no source is available |

Syntax is covered up to Java 25 (records, sealed types, pattern matching, switch expressions, text
blocks, module declarations, and so on).

## Building

Requires Rust 1.85+.

```sh
cargo build --release          # output: target/release/java-analyzer
cargo test --workspace         # unit tests do not need a JDK
cargo xtask dist               # build and collect into dist/
cargo xtask codegen            # regenerate AST node declarations after editing crates/syntax/ast.def
cargo bench -p syntax          # parsing and incremental reparsing benchmarks
cargo +nightly fuzz run parse  # fuzzing (needs cargo-fuzz; there is also a reparse target)
```

## Measurements

Release build on an 8-core / 31 GB Linux machine, running `java-analyzer parse` and
`java-analyzer analysis-stats` on the `src.zip` shipped with JDK 25 (15,227 files and 5.15 million
lines once unpacked):

| Item | Result |
|---|---|
| Parsing all files | 0.75 s, zero syntax errors, tree text byte-for-byte identical to the input |
| Building the symbol index | 4.3 s (320 thousand symbols) |
| Type inference for every method body plus all diagnostics | 27 s (multi-threaded wall-clock time) |
| Expressions with a known type | 99.9% |
| Error-level semantic diagnostics | 0 (any on valid code would be a false positive) |
| Completion | p50 0.05 ms, p95 1.2 ms |
| Hover / go to definition | p95 below 0.05 ms |
| Peak memory | 3.2 GB (inference results for every file are kept in memory; in an editor only open files are computed) |

Single-file parsing benchmark (`cargo bench -p syntax`): about 25 MiB/s for a whole file, about
50 µs for an incremental reparse inside a method body.

### Evals

`ja-eval` (see [evals/README.md](evals/README.md)) checks correctness against a real JDK. Results
are recorded in `evals/baseline.json`, and CI compares against it to prevent regressions:

| Item | Result |
|---|---|
| Scenario suite | 59 scenarios, all 450 expectations pass (navigation, references, hover, type inference, completion, signature help, diagnostics, flow analysis, nullness analysis, rename, code actions, inlay hints, Lombok, web endpoints) |
| javac differential: false positives | 0 (on the 13 corpus files as written and on the 122 mutants javac still accepts) |
| javac differential: detection rate | Across 18 mutation operators and 681 mutants, javac reports 648 errors of supported kinds; 602 are detected (93%), all with the matching category |
| Error-level diagnostics on the full JDK sources | 0 (every newly added kind of type check was confirmed to produce no false positives on these 5.15 million lines) |
| Formatting | For each of the 15,227 JDK files the token sequence is unchanged after formatting, and formatting a second time changes nothing |

The categories with a low detection rate are deliberately let through by the conservative strategy:
unimplemented abstract methods are matched only by name and parameter count (40%), override checks
do not cover generics or enum constant bodies (64%), and argument mismatches skip generic methods
and lambda arguments (84%). The 118 nullness warnings have no reference answer to compare against;
9 were inspected by hand and all were real problems.

These numbers describe this one corpus only: request latencies are sampled with warm caches and do
not cover the first request after a cold start, and nothing has been measured on application
projects with many third-party dependencies.

## Usage

Put `java-analyzer` on your `PATH` and configure your editor as described in
[docs/editors.md](docs/editors.md) (Neovim, Helix, Zed). Settings are documented in
[docs/configuration.md](docs/configuration.md).

A JDK must be installed on the machine at runtime (it is read for the standard library's type
information); JDK 9 and later as well as JDK 8 work.

Command-line tools:

```sh
java-analyzer parse <file or directory>       # parse and report syntax errors and timing
java-analyzer analysis-stats <project dir>    # import the project and analyze everything: per-phase timing, share of expressions with a known type
```

## Architecture

A layered cargo workspace: lower layers do not know about the layers above them, and no LSP type
appears in `ide` or below. See [docs/architecture.md](docs/architecture.md).

```
java-analyzer      LSP main loop, thread pool, protocol conversion, command line
ide                snapshot-based API: navigation, hover, hierarchies, highlighting, structure
ide-completion / ide-diagnostics
ide-db             database entities, symbol index, reference search, signature rendering, Javadoc
hir                semantic facade (Semantics, Definition)
hir-ty             types, subtyping, overload resolution, method body inference
hir-def            unified signature model (source and class files), class index, type name resolution
syntax / parser    lossless syntax trees, error-tolerant parsing
classfile          class / jar / ct.sym / jimage readers and the on-disk cache
project-model      JDK discovery, Maven, Gradle, manual configuration
base-db / vfs      inputs to incremental computation, file system abstraction
```

## Known limitations

- Lombok: members are synthesized only for the common annotations (`@Getter` `@Setter` `@Data`
  `@Value` `@With` `@Builder`, the constructor annotations and the logging annotations), and
  annotation arguments (access level, `fluent`, …) are not interpreted. Members generated by other
  annotation processors that do not write to disk are not recognized.
- Formatting never wraps or joins lines, and there is no code style configuration (the spacing
  rules are one fixed set); import ordering lives in "organize imports".
- Refactorings cover rename, extract local variable / method / constant, inline local variable,
  inline single-statement method, remove unused parameter, and move class (package and imports are
  updated when a `.java` file is moved in the editor); there is no general change-signature yet. An
  extracted method reuses the enclosing method's `throws` clause instead of computing its own
  exceptions, and does not handle the enclosing method's type parameters.
- Type checking is conservative and is not equivalent to javac: type mismatches and argument
  mismatches are only decided when both sides are primitive types or non-generic classes; generic
  bounds, unchecked conversions, initialization of `final` fields and module visibility are not
  checked. Unresolved type names are reported for simple names only, and not inside anonymous or
  local classes. Checked exceptions are not checked in code inside anonymous / local classes, nor
  for rethrows such as `throw e`. Missing `return` is not decided for methods with labeled jumps.
- Nullness analysis tracks only local variables and parameters and only reports dereferences that
  are definitely null; it does not know `@Nullable` / `@NotNull`, is not interprocedural, and
  treats fields and method return values as unknown.
- Quick fixes and refactorings are all single-step: methods / fields can only be created in the
  current class, moving and removing parameters is supported only for methods that take no part in
  overriding, and a method can only be pulled up to its direct superclass and must not use the
  subclass's own members. Refactorings that need a dialog (general change-signature, extract
  interface, move members to an arbitrary class) are not available.
- Spring: only endpoint annotations are recognized; bean injection, `@Value` properties and
  configuration files are not understood.
- Gradle import: only the "read the exported model" half has been verified; the injected init
  script has not been run against a real Gradle yet.
- The fuzz targets are written but have not actually been run.
- Local classes and variables of an anonymous class type such as `var x = new T() { ... }`: the
  members they declare themselves take no part in type checking or completion.
- Code lens shows counts only: standard LSP has no command that makes the client display a set of
  locations, so clicking does not navigate.
- Type inference for generic methods covers the common shapes only; when inference fails the type
  is "unknown", which produces no error but leaves the related completions missing.
- Members of local and anonymous classes are visible only within the enclosing method body and are
  not part of the global index.
- All modules share one global class namespace; classpaths are not isolated per module.
- In a multi-root workspace each folder is imported separately and merged into one project graph;
  module dependencies between roots are not established automatically, and the `[jdk]` section of
  `java-analyzer.toml` is taken from the first root.

## License

MIT or Apache-2.0, at your option.
