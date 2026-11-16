# java-analyzer

A Java language server written in Rust. Parsing, name resolution and type inference are all
implemented from scratch and do not depend on a JVM: the JDK and dependency jars are only read as
data to be indexed, and build tools are invoked only when importing a Gradle project (or when the
local Maven repository is missing artifacts).

> **Status: early development.** All unit tests and in-process end-to-end tests pass, and the whole
> JDK 25 source tree has been analyzed in full (see [Measurements](#measurements)); Maven and
> Gradle imports have been checked on real builds (Gradle 9, Groovy and Kotlin DSL). See
> [Known limitations](#known-limitations).

## Features

| Area | What is supported |
|---|---|
| Navigation | Go to definition / declaration / type definition / implementation, find references, document highlights, workspace symbols (also dependency and JDK types, matched by CamelCase abbreviations such as `CHM`), call hierarchy, type hierarchy; implementations and subtypes include local and anonymous classes (`new Runnable() {…}`, also with a diamond: `new Comparator<>() {…}`) and enum constant bodies, and rename updates their overrides; names in Javadoc (`{@link Foo#bar(int)}`, `{@linkplain}`, `{@value}`, `@see`, `@throws`, `@param`) resolve for go to definition, hover, highlights and references, and are updated by rename |
| Completion | Members, locals, static members, types (auto-import for types not yet imported), keywords, snippets and context-aware templates (`soutv` / `soutp` / `soutm` print the nearest variable, the method parameters or the method name; `iter` / `itar` / `ritar` loop over the nearest iterable or array with the element type and name filled in; `souf`, `thr`, `psfs` …), postfix templates (`.var` `.if` `.else` `.for` `.iter` `.itar` `.fori` `.forr` `.nn` `.inst` `.reqnonnull` `.synchronized` `.assert` `.arg` `.format` `.stream` `.opt` `.switch` `.try` `.serr` …), override stubs, Javadoc tags; by expected type: implementations after `new` (`List` → `ArrayList`, workspace implementations), lambda templates for functional interfaces, annotations filtered by their `@Target`; inside Javadoc: types after `{@link}` / `@see` / `@throws`, members after `#`, parameter types inside `{@link #foo(…)}` (whole overload signatures first), parameter and type parameter names after `@param` and block / inline tags, constants of the expected type (`TimeUnit.SECONDS`, `BigDecimal.ZERO`); ranked by expected type, scope distance and in-file usage frequency |
| Information | Hover (signature + Javadoc; on a call, the signature with the receiver's type arguments and the inferred method type arguments substituted, e.g. `Integer get(Object key)` on a `Map<String, Integer>`, followed by the declared one; also for library classes that ship sources — missing sources jars are downloaded in the background on first navigation; values of constant fields and `final` locals, evaluated from constant expressions in source or read from class files), signature help, semantic tokens (full / range / delta) |
| Structure | Document symbols, folding (including `//region` and `<editor-fold>` comments), selection ranges |
| Editing | Rename (including override chains, constructors, the matching file and Lombok accessors); formatting (indentation, spacing within a line, blank lines to keep and between members, optionally the placement of `{`; configured through `.editorconfig`: indent style and size, continuation indent, `case` and top-level member indentation, blank lines, spacing around operators, before and within parentheses, brace style, final newline — keys listed in [docs/configuration.md](docs/configuration.md), editor tab settings apply where `.editorconfig` sets nothing) and on-type formatting (indent on Enter, re-layout of the current line after `}` and `;`, a Javadoc skeleton with `@param` / `@return` / `@throws` after typing `/**` and Enter above a declaration); inlay hints (types of `var`, parameter names, types along multi-line method chains); code lens (reference and implementation counts; clicking lists them in VS Code, or through a configurable client command); a newly created empty `.java` file is filled with its package declaration and a class of the same name |
| Quick fixes | Import, create method / field / local variable, create a method or field in another class (`other.foo(x)`, `Util.foo(x)`, `other.count`, `Util.LIMIT`; an abstract method or constant in an interface) or a constructor for `new Foo(a, b)` without a matching one, adding the imports the target file needs, remove an extra argument or add a matching parameter to a `private` / `static` method (other calls get a default value), change a parameter type of such a method to the argument's type when the other calls still fit, create class / interface / enum / record / annotation in a new file, add `throws` or surround with try/catch, add a cast, add missing enum `switch` cases, initialize a variable, replace string `==` with `equals()`, remove unused variables / fields / methods (keeping side effects of the initializer), widen the visibility of an inaccessible member, remove `final` from an assigned variable, change a variable or return type to match the value, add a missing `return`, make a class abstract |
| Generation and refactoring | Organize imports (and import insertion) following the import layout and the on-demand import threshold of `.editorconfig` (`ij_java_imports_layout`, `ij_java_class_count_to_use_import_on_demand`, `ij_java_names_count_to_use_static_import_on_demand`); fix all behavior-preserving problems of a file at once (`source.fixAll`, e.g. on save); generate constructor / getter / setter / `toString` / `equals` / `hashCode` / delegate methods for a field; generate a builder (a static nested `Builder` with one chained setter per parameter and `build()`, plus `builder()`) from a constructor or record; implement abstract methods; add `@Override`; extract local variable / method (with the exceptions it actually throws and the type parameters it uses) / constant / field; encapsulate a field (make it private, generate accessors and rewrite reads and writes outside the class into accessor calls); inline constant; inline local variable / single-statement method; remove unused parameter; move parameter; introduce parameter (a selected expression becomes a new parameter of a `private` / `static` method, and every call passes it with the method's parameters replaced by the call's arguments); pull a method up to any workspace superclass; extract interface (public instance methods into a new interface the class implements, marking them `@Override`); move class and package (moving or renaming a `.java` file or a whole directory in the editor updates the package declarations and the imports that refer to the moved classes; classes moved together do not import each other); convert anonymous class to lambda; invert `if`; split `if (a && b)` into nested `if`s and merge them back; surround selected statements with `if` / `if-else` / `while` / `try-catch` / `try-finally` / `synchronized` / a block; replace `var` with an explicit type and an explicit type with `var` (only when the initializer has exactly the declared type and is not inferred from the target); add braces; split / join declaration and assignment; flip comparisons and `equals()`; De Morgan (`a && b` ↔ `!(!a || !b)`); remove an `else` after a branch that ends with `return` / `throw` / `break` / `continue`; `?:` ↔ if / else; lambda ↔ method reference; indexed `for` → enhanced `for`; old-style `switch` → `->` cases; a `switch` whose branches all assign the same variable or all return → a `switch` expression (merged into a preceding declaration without initializer); `if` / `else if` chains comparing one value with constants, enum constants or strings → `switch`; convert a data class (private final fields set by one constructor) to a `record`, keeping validation in a compact constructor; string concatenation → `String.format(...)`, or → a text block when it is made of string literals spanning several lines; create a field for a constructor parameter |
| Diagnostics | Pushed or pulled per file, and for the whole project through `workspace/diagnostic` (errors and warnings of closed files too, for the client's problems view; on by default up to 3,000 source files); syntax errors, with recovery for code being typed (a block missing its `}` ends at the next member declaration instead of swallowing the rest of the class, and type errors are not reported in statements that are still incomplete); type checking (unresolved symbols / types / imports, type mismatches, arguments not matching any overload, ambiguous calls, operator operands, non-boolean conditions, duplicate local variables, assignment to `final`, access to private members, instance members referenced from a static context or through the class name (`List.size()`), instantiation of abstract classes and interfaces, impossible casts and `instanceof` tests, members accessed on primitive values, `throw` of a non-`Throwable`, array initializer / index / dimension types, `case` constants of the wrong type, `var` without an inferable type, for-each over a value that is neither an array nor `Iterable`, lossy narrowing to `byte` / `short` / `char`, primitive or wrongly counted type arguments, outer type parameters used in a static context, an implicit `super()` without a matching superclass constructor, blank `final` fields not initialized by every constructor, `catch` of a checked exception the `try` block never throws, lambdas that do not fit their functional interface (wrong number of parameters, a value returned where none is expected or missing, a return value of the wrong type), `break` / `continue` outside a loop or with an unknown label, duplicate `case` labels, unimplemented abstract methods, invalid overrides, subtypes not permitted by a `sealed` type, type arguments outside their bounds, annotations on declarations their `@Target` does not allow, non-exhaustive `switch` and dominated `case` labels, unresolved modules in `module-info.java` and `import module`, rules for compact source files, language features above the module's language level or preview features used without `--enable-preview`); flow analysis (unreachable statements, missing `return`, reads of uninitialized variables, assigned values that are never read, unhandled checked exceptions, including code in anonymous and local classes and precise rethrows of a caught exception); inspections (null dereference, null checks that are always true / false, nullness annotations (`@Nullable` / `@NotNull` from JetBrains, JSR-305, JSpecify, Checker Framework, Android, Jakarta, Lombok — in source and in class files, including type annotations): dereferencing a value that may be null, passing null to a `@NotNull` parameter, returning null from a `@NotNull` method, with annotations inherited from overridden methods and `@NullMarked` / `@ParametersAreNonnullByDefault` style defaults; unused imports / locals / private members / parameters of private methods, strings and boxed numbers (`Integer`, `Long` …) compared with `==`, `equals()` between inconvertible types and collection / map lookups with an argument that can never be an element or key, format strings of `String.format` / `formatted` / `printf` with unknown conversions, a wrong number of arguments or mismatched argument types, ignored results of calls on immutable values (`s.trim();`, `amount.setScale(2);`, `date.plusDays(1);`) and stream pipelines without a terminal operation, arrays compared with `equals()` or turned into strings (`"x" + array`, `println(array)`, `array.toString()`), self assignment / comparison, loops whose condition only uses locals that the loop never updates, methods that always call themselves before they can return, `return;` / `continue;` at the end of a method or loop body, collections (locals and private fields) whose contents are only updated or only queried, integer division used as a floating-point value, octal literals, `equals()` without `hashCode()`, calls to `Thread.run()`, private methods that can be `static`, lambdas that can be method references, `Optional.get()` without an `isPresent()` check, checked exceptions declared in `throws` but never thrown, unnecessary boxing and unboxing (`Integer.valueOf(i)` / `new Integer(i)` where an `Integer` is expected, `n.intValue()` where an `int` is expected), explicit type arguments of `new` that `<>` would infer from the target type, C-style array declarations (`int a[]`), unnecessary `toString()` calls (on a `String`, or in string concatenation), redundant `new String(...)`, casts right after an `instanceof` test that can be pattern variables (`if (o instanceof Foo) { Foo f = (Foo) o;` → `if (o instanceof Foo f)`), `Map` code that can use `getOrDefault` / `putIfAbsent` / `computeIfAbsent`, `try` / `finally` blocks that only close a resource and can be try-with-resources, empty `catch` / `finally` blocks, `if` / `else` / side-effect-free `while` with a stray `;` as body, strings concatenated across loop iterations (with a fix that turns the variable into a `StringBuilder`), use of deprecated APIs (shown struck through), comparisons with `true` / `false`, `if` statements that return or assign their condition, `size() == 0` instead of `isEmpty()`, redundant casts, locals that are returned right after their declaration, redundant modifiers such as `public` on interface members or `static` on nested enums, raw use of generic classes, Stream call chains that can be simplified (`filter().findFirst().isPresent()` → `anyMatch()`, `stream().forEach()` → `forEach()`), `Collections.sort()` → `List.sort()`, `indexOf() != -1` → `contains()`, `String.valueOf()` in concatenation, private fields that may be `final` or can be local variables, exceptions created but not thrown, `synchronized` on a non-final field, non-atomic `++` / `+=` on a volatile field, int multiplication or shift used as `long`, redundant array creation for varargs, string concatenation inside `StringBuilder.append()`, unused labels, identical `if` / `else` branches and duplicate conditions in an `else if` chain), most with a quick fix; warnings can be suppressed with `@SuppressWarnings` (javac names such as `unused` / `deprecation`, inspection names such as `DataFlowIssue`, or `all`) or a `//noinspection` comment, and a quick fix adds the suppression to the statement, variable, field, method or class |
| Frameworks | Member synthesis for Lombok (accessors with `AccessLevel`, `@Accessors(fluent / chain / prefix)`, the constructor annotations with `staticName` / `access`, `@Builder` with its options, `@Builder.Default` and `@Singular`, `@Builder` on constructors and static methods, builders of generic classes, records and methods carrying their type parameters (`Pair.<String, Integer>builder().first("a").build()` is a `Pair<String, Integer>`), `@SuperBuilder` with the generated builder hierarchy (setters inherited from the parent's builder keep returning the subclass builder, `build()` returns the subclass, `toBuilder` when requested), `@With`, `@ToString` / `@EqualsAndHashCode`, `@FieldNameConstants`, `@StandardException`, `@FieldDefaults`, `@UtilityClass`, `@Delegate`, `val` / `var`, the logging annotations; `lombok.config` files apply to the sources in their directory and below, nearer files overriding farther ones); Spring MVC / JAX-RS endpoints, searchable by path through workspace symbols (`/api/users`, `GET /orders`); OpenFeign clients and Spring HTTP interfaces linked to the matching endpoints across services (go to definition / implementation, references from the endpoint, hover listing the target), preferring the service whose `spring.application.name` matches; Spring configuration keys from `application*.properties` / `.yml` in `@Value("${...}")` and `Environment.getProperty(...)`: completion, go to definition, hover with the value, and a warning for undefined keys without a default; inside `application*.properties` / `.yml` themselves: completion of keys and values, hover, go to definition and warnings for unknown or deprecated keys, from `spring-configuration-metadata.json` in dependencies and build output and from `@ConfigurationProperties` classes in the workspace |
| Projects | Maven (including `test-jar` dependencies and Eclipse PDE / Tycho bundles), Gradle, manual configuration via `java-analyzer.toml`; per-module language level and preview setting; automatic JDK discovery |
| Libraries | Going to a dependency class opens the source from the sources jar / `src.zip`, or, when no source is available, a stub generated from the class file with method bodies decompiled from bytecode (lambdas, anonymous classes, loops, `switch`, `try` / `finally`, `synchronized`; setting `decompiler`, also available as `java-analyzer decompile`); inside these read-only files, navigation, hover, references and highlights work for members, locals and calls as in project files (references on a declaration in a stub find the call sites in the project) |
| Debugging | `java-analyzer dap` is a Debug Adapter Protocol server speaking JDWP directly: launch (class path computed from the project model) or attach, line / conditional / hit-count / log breakpoints, exception breakpoints, stepping, threads and stack frames with sources from the workspace, sources jars and `src.zip`, variables (fields, array pages, strings, boxed values, enum names), setting variables and evaluating variables, field chains, array elements and comparisons; see [docs/editors.md](docs/editors.md) for nvim-dap, Helix and Zed |

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
| Diagnostics of the current file after an edit inside a method body | p50 about 6 ms, p95 35–100 ms (only the edited body is checked again; the rest comes from per-body caches) |
| Peak memory | 4.0 GB (inference results for every file are kept in memory; in an editor only open files are computed) |

Single-file parsing benchmark (`cargo bench -p syntax`): about 25 MiB/s for a whole file, about
50 µs for an incremental reparse inside a method body.

### Evals

`ja-eval` (see [evals/README.md](evals/README.md)) checks correctness against a real JDK. Results
are recorded in `evals/baseline.json`, and CI compares against it to prevent regressions:

| Item | Result |
|---|---|
| Scenario suite | 246 scenarios, all 1900 expectations pass (navigation, references, hover, type inference, completion, signature help, diagnostics, flow analysis, nullness analysis, rename, code actions, formatting, inlay hints, Lombok, web endpoints, project layout with test roots and library scopes, browsing library sources, modules, compact source files, language levels) |
| javac differential: false positives | 0 (on the 16 corpus files as written and on the mutants javac still accepts) |
| javac differential: detection rate | Across 21 mutation operators and 939 mutants, javac reports 934 errors of supported kinds; 896 are detected (96%), all with the matching category |
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

User-facing texts are in Chinese when the language starts with `zh` and in English otherwise: the
language server follows the client's `locale`, the command-line tools follow `LC_ALL` /
`LC_MESSAGES` / `LANG`.

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

- Lombok: of `lombok.config`, only the accessor (`chain`, `fluent`, `prefix`, `getter.noIsPrefix`),
  logger field name and `config.stopBubbling` keys are applied, and `import` lines are ignored.
  Members generated by other annotation processors that do not write to disk are not recognized.
- Formatting never wraps or joins lines (except moving `{` when a brace style is configured);
  import ordering lives in "organize imports".
- There is no general change-signature refactoring; only the common cases are covered by quick
  fixes (add a parameter for an extra argument, change a parameter type to the argument's type,
  remove an unused parameter, move a parameter), and only for methods that take no part in
  overriding.
- Type checking is conservative and is not equivalent to javac: a type or argument mismatch is
  reported only when every type involved is known — including parameterized types, wildcards
  (bounds and capture), type variables and generic calls or diamonds typed by their target — and
  never when a raw type, `var`, an implicitly typed lambda parameter without a definite target,
  a member reached through a raw supertype or an unsolved inference variable is involved.
  Unchecked conversions and module visibility are not checked; a mismatch in the result of a block
  lambda is reported on its `return` rather than on the call; reads of a `final` field
  before a constructor assigns it are not detected. Unresolved type names are reported for simple
  names only, and not inside diamond anonymous classes (`new T<>() { ... }`). The thrown types of
  `throw` with a method call or cast operand are not computed precisely.
- Nullness analysis tracks locals, parameters and `@Nullable` fields of `this` (or static ones)
  within a method; fields of other objects and `@Nullable` method calls fall back to a heuristic
  (not reported once the body compares them with null anywhere). Annotations are inherited from
  overridden methods, and scope defaults (`@NullMarked` / `@NullUnmarked`, JSR-305
  `@ParametersAreNonnullByDefault`, Spring `@NonNullApi`, Checker `@DefaultQualifier`) are applied
  to source code only, not to classes from jars; Eclipse `@NonNullByDefault` is not supported.
- Quick fixes and refactorings are all single-step, without a dialog: a pulled-up method must not
  use members of the classes it leaves, extract interface takes every public instance method that
  is not already an override, and general change-signature or moving members to an arbitrary class
  are not available.
- Spring: endpoints, configuration keys and configuration metadata are recognized; bean injection
  is not. Only the common YAML subset (nested maps, lists, block scalars, multiple documents) is
  read, without anchors or flow syntax; keys below `List<Pojo>` / `Map<String, Pojo>` properties are
  neither completed nor checked, and metadata is collected from the whole class path rather than
  per module.
- Library sources are analyzed against the project's class path rather than the library's own: when
  the project targets an older release through `ct.sym`, private members of a JDK class are taken
  from its opened source, but JDK-internal classes that `ct.sym` leaves out (`jdk.internal.*`)
  stay unresolved there.
- Generic inference covers target typing, lambdas and method references (bound from the target
  before their bodies are inferred) and collector / comparator chains; generic calls without a
  target and without arguments (`ImmutableList.of()`, `Ordering.natural()`, a diamond without a
  target) stay "unknown", which produces no error but leaves the related completions missing;
  when such a call is only used as the receiver of another call or as a statement, its unresolved
  type parameters take their declared bound (`<T extends S> T value(...)` gives `S`).
- Class visibility follows the module dependency graph, and a class defined in several modules or
  jars under the same name is resolved to the copy visible from each file. Types in the signatures
  of another module are resolved against that module rather than the using one, duplicate classes
  on the class path use the first jar's copy for stubs and library subtype tables, and Spring
  configuration files resolve types globally. Generated sources are not split into main and test.
- Parameter names and Javadoc for library classes come from the sources jar / `src.zip`. A missing
  sources jar is downloaded when navigating into the library (through `mvn`, or `curl` from the
  configured mirror / Maven Central), only for jars whose coordinates follow from their path in the
  Maven repository or Gradle cache; until then, and for other jars, parameters of methods that have
  no body in the class file show up as `arg0`, `arg1`, …, and the class opens as a decompiled stub. The decompiler leaves a method body empty with a comment when it cannot structure the bytecode (about 0.4% of JDK methods, such as pattern-matching `switch`), does not fold varargs arrays and shows constants as literals.
- In a multi-root workspace each folder is imported separately and merged into one project graph;
  Maven and Gradle modules are linked across roots by their coordinates, Eclipse projects are not.
  Gradle composite builds (`includeBuild` with a literal path) are imported, and `buildSrc` is
  imported as a plain module — build scripts themselves are not analyzed.
- HTTP clients (`@FeignClient`, `@HttpExchange` interfaces) are matched to server endpoints by
  method and path, including shared contract interfaces (mappings declared on an interface that
  controllers implement and Feign clients extend; interfaces are linked by simple name). Finding
  references on a client method does not list the server method.
- The debugger does not build the project (launch needs compiled output). Evaluation covers
  variables, field chains, array elements and comparisons, not method calls or arithmetic;
  collections show their fields rather than a logical view; hot code replace, function and data
  breakpoints are not supported.

## License

MIT.
