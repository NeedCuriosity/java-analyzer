# Measurements

## JDK 25 sources

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
| Diagnostics of the current file after changing a method signature or a field type | p50 about 5–7 ms, p95 about 40 ms (only the bodies that use the changed member are inferred and checked again) |
| Peak memory | 4.0 GB (inference results for every file are kept in memory; in an editor only open files are computed) |

Single-file parsing benchmark (`cargo bench -p syntax`): about 25 MiB/s for a whole file, about
50 µs for an incremental reparse inside a method body.

## Application projects

Open-source Maven and Gradle projects that compile with JDK 25 (so every error-level diagnostic
would be a false positive), analyzed with `java-analyzer analysis-stats` after clearing the
analyzer's class index cache, on the same machine. For the Gradle projects the exported Gradle
model was cached, so the import time does not include running Gradle:

| Project | Modules / libraries | Files (lines) | Import | Class path | Index | Inference + diagnostics | Known types | Peak memory | Errors |
|---|---|---|---|---|---|---|---|---|---|
| spring-petclinic | 1 / 171 | 50 (4.3 k) | 0.02 s | 0.29 s | 0.06 s | 0.05 s | 99.2% | 352 MB | 0 |
| mall | 8 / 207 | 523 (85 k) | 0.06 s | 0.56 s | 0.22 s | 0.39 s | 99.8% | 536 MB | 0 |
| RuoYi-Vue | 7 / 144 | 266 (34 k) | 0.05 s | 0.22 s | 0.10 s | 0.16 s | 99.9% | 552 MB | 0 |
| pig (Spring Cloud) | 24 / 612 | 559 (49 k) | 0.16 s | 0.52 s | 0.19 s | 0.20 s | 99.4% | 766 MB | 0 |
| eladmin | 6 / 246 | 293 (25 k) | 0.05 s | 0.38 s | 0.14 s | 0.11 s | 99.4% | 631 MB | 0 |
| mockito (Gradle, Kotlin DSL) | 24 / 117 | 1,015 (103 k) | 0.003 s | 0.67 s | 0.23 s | 0.54 s | 99.5% | 622 MB | 0 |
| resilience4j (Gradle, Groovy DSL) | 30 / 386 | 1,233 (132 k) | 0.015 s | 0.36 s | 0.26 s | 1.09 s | 99.2% | 758 MB | 0 |

Peak memory covers the whole run, including the sampled completion and request latencies.

## Evals

`ja-eval` (see [evals/README.md](../evals/README.md)) checks correctness against a real JDK. Results
are recorded in `evals/baseline.json`, and CI compares against it to prevent regressions:

| Item | Result |
|---|---|
| Scenario suite | 274 scenarios, all 2085 expectations pass (navigation, references, hover, type inference, completion, signature help, diagnostics, flow analysis, nullness analysis, rename, code actions, formatting, inlay hints, Lombok, web endpoints, Spring bean injection, project layout with test roots and library scopes, browsing library sources, modules, compact source files, language levels) |
| javac differential: false positives | 0 (on the 18 corpus files as written and on the mutants javac still accepts; this includes `unchecked` warnings, which must fall on lines where `javac -Xlint:unchecked` warns) |
| javac differential: detection rate | Across 23 mutation operators and 1130 mutants, javac reports 1086 errors of supported kinds; 1056 are detected (97%), 1055 with the matching category. Of the 48 lines in the unmodified corpus where javac reports `unchecked` warnings, 45 are detected |
| Error-level diagnostics on the full JDK sources | 0 (every newly added kind of type check was confirmed to produce no false positives on these 5.15 million lines) |
| Formatting | For each of the 15,227 JDK files the token sequence is unchanged after formatting, formatting a second time changes nothing, and no closing brace that was aligned with its statement is moved out of alignment |

The categories with a lower detection rate are deliberately let through by the conservative strategy:
unimplemented abstract methods and invalid overrides are not checked in anonymous classes created
with a diamond (`new Comparator<>() {…}`; 87% and 97%); argument mismatches of generic methods (98% for
all calls) are reported only for a violated bound, contradicting constraints on a type parameter from
arguments of known type, or a lambda of the wrong shape. The 118 nullness warnings have no reference answer to compare against;
9 were inspected by hand and all were real problems.

These numbers describe this one corpus only: request latencies are sampled with warm caches and do
not cover the first request after a cold start.
