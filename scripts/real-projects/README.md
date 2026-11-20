# Real-project checks

Unit tests and `ja-eval` use small, synthetic projects. These checks import real open-source
Maven projects the way a user's editor would and compare the result with what Maven itself does:

| check       | what it verifies |
|-------------|------------------|
| `classpath` | For every module, the runtime class path from `java-analyzer/projectClasspath` against `mvn dependency:build-classpath -DincludeScope=runtime` (run as `mvn compile dependency:build-classpath ...` over the reactor, which also compiles for `launch`) plus the `system`-scope jars from `-DincludeScope=system`, which Maven leaves out of the runtime scope but a launched program needs: duplicates (same `group:artifact` twice), missing and extra entries, version differences. Workspace modules count as the same entry whether they appear as `target/classes` or as an installed jar. Also checks that the chosen JDK matches the language level: JDK 25 is `JAVA_HOME`, the other JDKs are only discoverable in `~/.jdks`. |
| `launch`    | Starts the main class through the debug adapter (`java-analyzer dap`, `launch` without `classPaths`), so the JDK, class path and VM arguments are exactly the ones a user gets. Passes on Spring's `Started ... in` line. A failure caused by MySQL / Redis not running is accepted once Spring has started loading the context; `ClassNotFoundException`, `NoSuchMethodError` and other linkage errors fail. The adapter's JDK and class path must equal `projectClasspath`'s. |
| `settings`  | A `settings.xml` whose `<localRepository>` is a copy of the repository somewhere else, passed as `maven.settings`, with no `~/.m2/repository` at all: import must resolve everything without running `mvn` (checked in the `JA_LOG=project_model=info` log) and give the same class paths; the launch must use it too. |
| `gbk`       | A generated project with GBK-encoded sources (Chinese comments, string literals, identifiers) and `project.build.sourceEncoding=GBK`, at language levels 8 and 21: no error diagnostics, goto definition into an unopened GBK file lands on the right column, and the launched program's output reaches the debug client intact. Uses the `zh_CN.GBK` locale when it is installed. |
| `stats`     | `java-analyzer analysis-stats` on every project exits normally (no panic). |

Projects (pinned commits in `realproj.py`): spring-petclinic, RuoYi-Vue, mall, eladmin, pig,
commons-io 2.4, plus the generated `system-scope-demo`: two modules using jars under `lib/` through
`system` scope, one declared directly and one through the parent's `dependencyManagement`.

## Running

```sh
scripts/real-projects/run.sh            # all checks
scripts/real-projects/run.sh classpath launch
JA_REALPROJ_ONLY=mall,eladmin scripts/real-projects/run.sh
```

Linux only. Needs `git`, `curl`, `python3` and network access for the first run. `run.sh`
downloads Maven 3.9.9 and Temurin JDKs 8, 11, 17, 21, 25, builds `java-analyzer` in release mode,
clones the projects and lets Maven fill an isolated local repository. Everything goes under
`$JA_REALPROJ_WORK` (default `~/.cache/ja-realproj`, a few GB); the real `~/.m2` and user
configuration are never used. The report is printed and saved to `$JA_REALPROJ_WORK/report.txt`,
per-project logs (Maven, server, debug adapter, program output) are in `$JA_REALPROJ_WORK/logs`.
The exit status is non-zero when any check fails.

Environment:

| variable | meaning |
|----------|---------|
| `JA_REALPROJ_WORK` | work directory |
| `JA_BIN` | use this `java-analyzer` binary instead of building one |
| `JA_JDK_8`, `JA_JDK_11`, `JA_JDK_17`, `JA_JDK_21`, `JA_JDK_25` | use an existing JDK instead of downloading |
| `JA_JDK_6` | also make a JDK 6 discoverable (commons-io 2.4 targets Java 6) |
| `JA_MAVEN_MIRROR` | mirror of Maven Central for the isolated `settings.xml` |
| `JA_MAVEN_MIRROR_ID` | the mirror's id (default `mirror`); Maven only reuses cached artifacts downloaded through a repository with the same id |
| `JA_MAVEN_PROXY` | `host:port` of an HTTP proxy for Maven |
| `JA_REALPROJ_ONLY` | comma-separated project names |
| `JA_LAUNCH_TIMEOUT` | seconds to wait for an application to start (default 90) |
