# 配置

## 服务器选项

通过 `initializationOptions` 或 `workspace/didChangeConfiguration` 传入。
可以直接给出下面的对象，也可以包在 `"java-analyzer"` 键下。

| 键 | 默认值 | 说明 |
|---|---|---|
| `jdkHome` | 自动发现 | JDK 的安装目录。未设置时依次查找 `JAVA_HOME`、`/usr/libexec/java_home`、SDKMAN、`~/.jdks`、Gradle toolchain、mise / asdf |
| `maven.useCli` | `true` | 本地仓库缺少依赖或 POM 无法静态解析时，是否调用 `mvn`（优先 `./mvnw`）导出 classpath |
| `gradle.enabled` | `true` | 是否允许运行 Gradle 导入工程。关闭后只使用上一次导入的缓存 |
| `completion.autoImport` | `true` | 选中未导入的类型时自动添加 import；关闭后插入全限定名 |
| `completion.unimportedTypes` | `true` | 是否在候选里包含未导入的类型 |
| `completion.postfix` | `true` | 是否提供 `.var` / `.if` / `.for` / `.nn` 等后缀模板 |
| `completion.limit` | `400` | 一次返回的候选上限 |
| `codeLens.references` | `true` | 在声明上方显示引用数；客户端自己会显示用法提示时可以关掉 |
| `codeLens.implementations` | `true` | 在类型 / 方法上方显示实现数 |
| `codeLens.command` | 自动 | 点击 code lens 时执行的客户端命令，参数是 `[uri, position, locations]`；不设置时 VS Code 用 `editor.action.showReferences`，其它客户端只显示数量 |
| `diagnostics.semantic` | `true` | 是否报告语义诊断（无法解析的符号、类型不匹配等）；关闭后只有语法错误 |
| `diagnostics.unusedImports` | `true` | 是否提示未使用的 import |
| `diagnostics.unusedSymbols` | `true` | 是否提示未使用的局部变量和私有成员 |
| `diagnostics.workspace` | 自动 | 是否计算整个工程的诊断（`workspace/diagnostic`，客户端的问题列表用），只含错误和警告；不设置时，源文件不超过 3000 个才计算 |
| `diagnostics.inspections` | `true` | 是否做流分析（不可达语句、缺少 `return`、未初始化的局部变量、未处理的受检异常）和表达式检查（`==` 比较字符串、自我赋值、自我比较） |

修改 `jdkHome`、`maven.*`、`gradle.*` 会触发重新导入工程。

## 工程配置：`java-analyzer.toml`

放在工作区根目录。用于没有 Maven / Gradle 的工程，或覆盖自动发现的 JDK。

```toml
[jdk]
home = "/Library/Java/JavaVirtualMachines/temurin-21.jdk/Contents/Home"

[[module]]
name = "app"
root = "."                       # 相对工作区根目录，默认就是根目录
sources = ["src"]
test_sources = ["test"]
classpath = ["lib/*.jar", "/opt/libs/guava.jar"]
language_level = 21
preview = false                  # 是否开启预览特性（相当于 javac --enable-preview）
```

只要存在 `[[module]]`，就不再尝试 Maven / Gradle 导入。只写 `[jdk]` 时构建系统照常导入。

### 语言级别和预览特性

每个模块的语言级别决定哪些语法可以用：比如语言级别 8 的模块里写 record、`var`、文本块
会报 `feature-not-available`。语言级别来自：

- Maven：`maven-compiler-plugin` 的 `<release>` / `<source>`，或 `maven.compiler.release` /
  `maven.compiler.source` / `java.version` 属性；
- Gradle：`java.sourceCompatibility`；
- `java-analyzer.toml`：`language_level`。

都没有时按 JDK 的版本算。预览特性（例如模式里的基本类型）只有在模块开启了预览、并且语言级别
等于 JDK 版本时才能用，和 javac 的要求一致。开启预览的方式：

- Maven：`maven-compiler-plugin` 的 `<enablePreview>true</enablePreview>`，或 `<compilerArgs>`
  里有 `--enable-preview`，或属性 `maven.compiler.enablePreview=true`；
- Gradle：`JavaCompile` 任务的 `options.compilerArgs` 里有 `--enable-preview`；
- `java-analyzer.toml`：`preview = true`。

## 工程导入的行为

- **Maven**：直接读取 POM（父 POM、属性、dependencyManagement、BOM、scope、exclusion、
  多模块），依赖从 `~/.m2/repository` 解析，不联网。
- **Gradle**：通过 init script 让 `./gradlew`（没有 wrapper 时用 `gradle`）导出各 source set
  的源码目录和编译 classpath（每个子工程一个导出任务 `javaAnalyzerModel`，只解析依赖、不编译；
  Groovy 和 Kotlin DSL 都可以，已在 Gradle 9 上验证）。结果按构建文件内容缓存，构建文件不变时
  启动不会运行 Gradle。
- **都没有**：按目录约定找源码根（`src/main/java`、`src/test/java`、`src`）。
- `pom.xml`、`*.gradle(.kts)`、`gradle.properties`、`libs.versions.toml`、`java-analyzer.toml`
  变化后自动重新导入。
- `target/generated-sources/*`、`build/generated/sources/*` 下的代码作为源码参与分析。

## 多根工作区

客户端传入多个 workspace folder 时，每个文件夹按上面的规则各自导入，再合并：模块全部保留，
同一个 jar 只索引一次。JDK 取第一个在 `java-analyzer.toml` 里写了 `[jdk]` 的文件夹，都没写时取第一个
找到的。一个文件夹里的模块通过 Maven 坐标（`groupId:artifactId`）依赖另一个文件夹里的 Maven 模块时，
会改成直接依赖那个模块的源码，而不是本地仓库里的 jar；其它构建工具之间不会自动建立模块依赖。
运行中增删文件夹（`workspace/didChangeWorkspaceFolders`）会触发重新导入。

## 缓存

都在系统缓存目录的 `java-analyzer/` 下（Linux：`~/.cache`，macOS：`~/Library/Caches`）：

- `index/`：类路径索引；
- `gradle/`：Gradle 导入结果；
- `sources/`：依赖库的源码 / 签名桩。

缓存可以随时删除，下次启动会重建。
