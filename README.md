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
| Completion | Members, locals, static members, types (auto-import for types not yet imported), keywords, snippets and context-aware templates (`soutv` / `soutp` / `soutm` print the nearest variable, the method parameters or the method name; `iter` / `itar` / `ritar` loop over the nearest iterable or array with the element type and name filled in; `souf`, `thr`, `psfs` …), postfix templates (`.var` `.if` `.else` `.for` `.iter` `.itar` `.fori` `.forr` `.nn` `.inst` `.reqnonnull` `.synchronized` `.assert` `.arg` `.format` `.stream` `.opt` `.switch` `.try` `.serr` …), override stubs, Javadoc tags; by expected type: implementations after `new` (`List` → `ArrayList`, workspace implementations), lambda templates for functional interfaces, constants of the expected type (`TimeUnit.SECONDS`, `BigDecimal.ZERO`); ranked by expected type, scope distance and in-file usage frequency |
| Information | Hover (signature + Javadoc, also for library classes that ship sources; values of constant fields and `final` locals, evaluated from constant expressions in source or read from class files), signature help, semantic tokens (full / range / delta) |
| Structure | Document symbols, folding (including `//region` and `<editor-fold>` comments), selection ranges |
| Editing | Rename (including override chains, constructors, the matching file and Lombok accessors); formatting (indentation, spacing within a line, consecutive blank lines) and on-type formatting (indent on Enter, re-layout of the current line after `}` and `;`); inlay hints (types of `var`, parameter names, types along multi-line method chains); code lens (reference and implementation counts) |
| Quick fixes | Import, create method / field / local variable, create class / interface / enum / record / annotation in a new file, add `throws` or surround with try/catch, add a cast, add missing enum `switch` cases, initialize a variable, replace string `==` with `equals()`, remove unused variables / fields / methods (keeping side effects of the initializer), widen the visibility of an inaccessible member, remove `final` from an assigned variable, change a variable or return type to match the value, add a missing `return`, make a class abstract |
| Generation and refactoring | Organize imports; generate constructor / getter / setter / `toString` / `equals` / `hashCode` / delegate methods for a field; implement abstract methods; add `@Override`; extract local variable / method / constant / field; inline constant; inline local variable / single-statement method; remove unused parameter; move parameter; introduce parameter (a selected expression becomes a new parameter of a `private` / `static` method, and every call passes it with the method's parameters replaced by the call's arguments); pull method up to the superclass; convert anonymous class to lambda; invert `if`; split `if (a && b)` into nested `if`s and merge them back; surround selected statements with `if` / `if-else` / `while` / `try-catch` / `try-finally` / `synchronized` / a block; replace `var` with an explicit type; add braces; split / join declaration and assignment; flip comparisons and `equals()`; `?:` ↔ if / else; lambda ↔ method reference; indexed `for` → enhanced `for`; old-style `switch` → `->` cases; `if` / `else if` chains comparing one value with constants, enum constants or strings → `switch`; convert a data class (private final fields set by one constructor) to a `record`, keeping validation in a compact constructor; string concatenation → `String.format(...)`, or → a text block when it is made of string literals spanning several lines; create a field for a constructor parameter |
| Diagnostics | Syntax errors; type checking (unresolved symbols / types / imports, type mismatches, arguments not matching any overload, ambiguous calls, operator operands, non-boolean conditions, duplicate local variables, assignment to `final`, access to private members, instance members referenced from a static context, unimplemented abstract methods, invalid overrides, subtypes not permitted by a `sealed` type, type arguments outside their bounds, non-exhaustive `switch` and dominated `case` labels, unresolved modules in `module-info.java` and `import module`, rules for compact source files, language features above the module's language level or preview features used without `--enable-preview`); flow analysis (unreachable statements, missing `return`, reads of uninitialized variables, assigned values that are never read, unhandled checked exceptions); inspections (null dereference, null checks that are always true / false, nullness annotations (`@Nullable` / `@NotNull` from JetBrains, JSR-305, JSpecify, Checker Framework, Android, Jakarta, Lombok — in source and in class files, including type annotations): dereferencing a value that may be null, passing null to a `@NotNull` parameter, returning null from a `@NotNull` method; unused imports / locals / private members / parameters of private methods, strings and boxed numbers (`Integer`, `Long` …) compared with `==`, `equals()` between inconvertible types and collection / map lookups with an argument that can never be an element or key, format strings of `String.format` / `formatted` / `printf` with unknown conversions, a wrong number of arguments or mismatched argument types, ignored results of calls on immutable values (`s.trim();`, `amount.setScale(2);`, `date.plusDays(1);`) and stream pipelines without a terminal operation, arrays compared with `equals()` or turned into strings (`"x" + array`, `println(array)`, `array.toString()`), self assignment / comparison, loops whose condition only uses locals that the loop never updates, methods that always call themselves before they can return, `return;` / `continue;` at the end of a method or loop body, collections (locals and private fields) whose contents are only updated or only queried, integer division used as a floating-point value, octal literals, `equals()` without `hashCode()`, calls to `Thread.run()`, strings concatenated across loop iterations (with a fix that turns the variable into a `StringBuilder`), use of deprecated APIs (shown struck through), comparisons with `true` / `false`, `if` statements that return or assign their condition, `size() == 0` instead of `isEmpty()`, redundant casts, locals that are returned right after their declaration, redundant modifiers such as `public` on interface members or `static` on nested enums, private fields that may be `final`), most with a quick fix; warnings can be suppressed with `@SuppressWarnings` (javac names such as `unused` / `deprecation`, inspection names such as `DataFlowIssue`, or `all`) or a `//noinspection` comment, and a quick fix adds the suppression to the statement, variable, field, method or class |
| Frameworks | Member synthesis for common Lombok annotations (including `val` / `var`, `@Accessors(fluent / chain)`, `@UtilityClass`, `@Delegate` and `@SuperBuilder`); Spring MVC / JAX-RS endpoints, searchable by path through workspace symbols (`/api/users`, `GET /orders`) |
| Projects | Maven (including `test-jar` dependencies and Eclipse PDE / Tycho bundles), Gradle, manual configuration via `java-analyzer.toml`; per-module language level and preview setting; automatic JDK discovery |
| Libraries | Going to a dependency class opens the source from the sources jar / `src.zip`, or a generated signature stub when no source is available; inside these read-only files, navigation, hover, references and highlights work for members, locals and calls as in project files (references on a declaration in a stub find the call sites in the project) |

Syntax is covered up to Java 25 (records, sealed types, pattern matching, switch expressions, text
blocks, module declarations, module imports, compact source files, and so on), plus primitive types
in patterns, `instanceof` and `switch` as a preview feature.

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
| Scenario suite | 145 scenarios, all 1021 expectations pass (navigation, references, hover, type inference, completion, signature help, diagnostics, flow analysis, nullness analysis, rename, code actions, formatting, inlay hints, Lombok, web endpoints, project layout with test roots and library scopes, browsing library sources, modules, compact source files, language levels) |
| javac differential: false positives | 0 (on the 14 corpus files as written and on the mutants javac still accepts) |
| javac differential: detection rate | Across 18 mutation operators and 733 mutants, javac reports 699 errors of supported kinds; 657 are detected (94%), all with the matching category |
| Error-level diagnostics on the full JDK sources | 0 (every newly added kind of type check was confirmed to produce no false positives on these 5.15 million lines) |
| Formatting | For each of the 15,227 JDK files the token sequence is unchanged after formatting, formatting a second time changes nothing, and no closing brace that was aligned with its statement is moved out of alignment |

The categories with a low detection rate are deliberately let through by the conservative strategy:
unimplemented abstract methods are matched only by name and parameter count (40%), override checks
do not cover generics or enum constant bodies (64%), and argument mismatches skip generic methods
(except bound violations) and lambda arguments (87%). The 118 nullness warnings have no reference answer to compare against;
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
  `@Value` `@With` `@Builder` `@SuperBuilder` `@Accessors` `@UtilityClass` `@Delegate`, `val` / `var`,
  the constructor annotations and the logging annotations), and annotation arguments other than
  `@Accessors(fluent / chain)` (access level, …) are not interpreted. Members generated by other
  annotation processors that do not write to disk are not recognized.
- Formatting never wraps or joins lines, and there is no code style configuration (the spacing
  rules are one fixed set); import ordering lives in "organize imports".
- Refactorings cover rename, extract local variable / method / constant, inline local variable,
  inline single-statement method, remove unused parameter, and move class (package and imports are
  updated when a `.java` file is moved in the editor); there is no general change-signature yet. An
  extracted method reuses the enclosing method's `throws` clause instead of computing its own
  exceptions, and does not handle the enclosing method's type parameters.
- Type checking is conservative and is not equivalent to javac: type mismatches and argument
  mismatches are only decided when both sides are primitive types or non-generic classes (plus
  generic bound violations and target types that conflict with an argument); unchecked conversions, initialization of `final` fields and module visibility are not
  checked. Unresolved type names are reported for simple names only, and not inside anonymous or
  local classes. Checked exceptions are not checked in code inside anonymous / local classes, nor
  for rethrows such as `throw e`. Missing `return` is not decided for methods with labeled jumps.
- Nullness analysis tracks only local variables and parameters. Unannotated fields and method
  return values are unknown; annotated ones are taken from their declaration only (annotations are
  not inherited from overridden methods, and package / class defaults such as `@NullMarked` are not
  applied). A `@Nullable` field or method call is not reported once the method body compares it
  with null or tests it with `instanceof` anywhere, since fields are not tracked.
- Quick fixes and refactorings are all single-step: methods / fields can only be created in the
  current class, moving and removing parameters is supported only for methods that take no part in
  overriding, and a method can only be pulled up to its direct superclass and must not use the
  subclass's own members. Refactorings that need a dialog (general change-signature, extract
  interface, move members to an arbitrary class) are not available.
- Spring: only endpoint annotations are recognized; bean injection, `@Value` properties and
  configuration files are not understood.
- Gradle import: only the "read the exported model" half has been verified; the injected init
  script has not been run against a real Gradle yet.
- Library sources are analyzed against the project's class path rather than the library's own: when
  the project targets an older release through `ct.sym`, private members of a JDK class are taken
  from its opened source, but JDK-internal classes that `ct.sym` leaves out (`jdk.internal.*`)
  stay unresolved there.
- Local classes and variables of an anonymous class type such as `var x = new T() { ... }`: the
  members they declare themselves take no part in type checking or completion.
- Code lens shows counts only: standard LSP has no command that makes the client display a set of
  locations, so clicking does not navigate.
- Type inference for generic methods covers the common shapes only; when inference fails the type
  is "unknown", which produces no error but leaves the related completions missing.
- Members of local and anonymous classes are visible only within the enclosing method body and are
  not part of the global index.
- All modules share one global class namespace; classpaths are not isolated per module. The only
  separation is between main and test: code in a main source root cannot see classes from test
  source roots or from libraries its module depends on with test scope only.
- Parameter names and Javadoc for library classes come from the sources jar / `src.zip`; without
  one, parameters of methods that have no body in the class file show up as `arg0`, `arg1`, ….
- User-facing texts (diagnostics, code action titles, code lens) are in Chinese when the client's
  `locale` starts with `zh` and in English otherwise; command-line output is Chinese only.
- In a multi-root workspace each folder is imported separately and merged into one project graph;
  module dependencies between roots are not established automatically, and the `[jdk]` section of
  `java-analyzer.toml` is taken from the first root.

## License

MIT.
