# 编辑器接入

java-analyzer 只说标准 LSP，通过标准输入输出通信。任何支持 LSP 的编辑器都可以接入：
把 `java-analyzer` 放到 `PATH` 里，让编辑器在打开 `.java` 文件时启动它即可。

配置项通过 `initializationOptions` 传入（见 [configuration.md](configuration.md)）。

调试用的是另一个子命令 `java-analyzer dap`（Debug Adapter Protocol），见[调试](#调试debug-adapter-protocol)。

## VS Code

仓库里的 [`editors/code`](../editors/code) 是一个薄的 VS Code 扩展：启动 `java-analyzer`，把设置传给它，
其余都走标准 LSP。打包安装：

```sh
cd editors/code
npm ci
npm run package                                   # 生成 java-analyzer-<版本>.vsix
code --install-extension java-analyzer-*.vsix
```

- 服务器默认从 `PATH` 里找，也可以用 `java-analyzer.server.path` 指定；改了路径会自动重启服务器。
- [配置](configuration.md)里的选项都写成 `java-analyzer.*` 设置（如 `java-analyzer.jdkHome`、
  `java-analyzer.completion.autoImport`），修改后立即生效，不用重启。
- 除 `.java` 文件外，Spring Boot 的 `application*` / `bootstrap*` 配置文件（`.properties` / `.yml` / `.yaml`）
  也交给服务器。
- 点击声明上方的引用数 / 实现数，在 peek 视图里列出它们。
- 调试类型 `java-analyzer`（`launch` / `attach`，参数见[调试](#调试debug-adapter-protocol)），
  `launch.json` 里有现成的片段。
- 命令：**java-analyzer: Restart Server**、**java-analyzer: Show Output**（服务器日志在 *java-analyzer*
  输出面板，用 `java-analyzer.server.extraEnv` 设置 `JA_LOG` 调整日志级别）。

工作区里请停用其他 Java 语言扩展（如 Red Hat 的 Language Support for Java），否则两个服务器会同时响应。

## Neovim（0.11+，内置 LSP）

```lua
-- ~/.config/nvim/lsp/java_analyzer.lua
return {
  cmd = { "java-analyzer" },
  filetypes = { "java" },
  root_markers = {
    "java-analyzer.toml",
    "settings.gradle", "settings.gradle.kts",
    "pom.xml", "build.gradle", "build.gradle.kts",
    ".git",
  },
  init_options = {
    -- jdkHome = "/path/to/jdk",
    completion = { autoImport = true, postfix = true },
  },
}
```

```lua
-- init.lua
vim.lsp.enable("java_analyzer")
```

点击 code lens 列出引用 / 实现：在 `init_options` 里加 `codeLens = { command = "java-analyzer.showLocations" }`，
再注册这个命令，把位置放进 quickfix 列表：

```lua
vim.lsp.commands["java-analyzer.showLocations"] = function(command, ctx)
  local client = vim.lsp.get_client_by_id(ctx.client_id)
  local items = vim.lsp.util.locations_to_items(command.arguments[3], client.offset_encoding)
  vim.fn.setqflist({}, " ", { title = command.title, items = items })
  vim.cmd.copen()
end
```

Neovim 0.10 及更早版本可以用 `vim.lsp.start`：

```lua
vim.api.nvim_create_autocmd("FileType", {
  pattern = "java",
  callback = function(args)
    vim.lsp.start({
      name = "java-analyzer",
      cmd = { "java-analyzer" },
      root_dir = vim.fs.root(args.buf, { "pom.xml", "settings.gradle", "build.gradle", ".git" }),
    })
  end,
})
```

## Helix

```toml
# ~/.config/helix/languages.toml
[language-server.java-analyzer]
command = "java-analyzer"

[language-server.java-analyzer.config]
# jdkHome = "/path/to/jdk"
completion = { autoImport = true, postfix = true }

[[language]]
name = "java"
language-servers = ["java-analyzer"]
roots = ["java-analyzer.toml", "settings.gradle", "settings.gradle.kts", "pom.xml", "build.gradle"]
```

`hx --health java` 可以确认服务器是否被找到。

## Zed

Zed 里语言服务器由扩展注册，不能只靠设置文件接入一个全新的服务器。
本仓库不包含 Zed 扩展；需要在 Zed 里使用时，写一个最小的扩展，在
`language_server_command` 里返回 `java-analyzer` 的路径，然后在 `settings.json` 里启用它：

```json
{
  "languages": {
    "Java": { "language_servers": ["java-analyzer"] }
  },
  "lsp": {
    "java-analyzer": {
      "initialization_options": {
        "completion": { "autoImport": true }
      }
    }
  }
}
```

## Spring Boot 配置文件

服务器也能处理 `application*.properties` / `application*.yml` / `application*.yaml`
（以及 `bootstrap*`）：补全键（properties 里是完整的点分键，YAML 里是当前缩进层次下的下一段，
选中后带上 `key: ` 或换行缩进）和值（布尔、枚举、元数据里的提示值），悬停显示类型、默认值和说明，
跳转到绑定它的 `@ConfigurationProperties` 字段（依赖库里的配置项跳到声明它的类），
并对已弃用的键、已知分组下不存在的键给出警告（完全自定义的键不报）。

已知的键来自依赖 jar 和构建输出（`target/classes`、`build/...`）里的
`META-INF/spring-configuration-metadata.json`、手写的 `additional-spring-configuration-metadata.json`，
以及工作区里 `@ConfigurationProperties` 标注的类。

要用上这些功能，编辑器需要把这些文件也交给 java-analyzer。服务器只分析 Spring 配置文件，
其他 YAML / properties 文件即使被交过来也会被忽略，所以按文件类型整体挂上也没有副作用。

Neovim：在 `filetypes` 里加上 `yaml` 和 `jproperties`，并用 `root_dir` 只在
`application*` / `bootstrap*` 文件上启动（其他 YAML 文件不挂这个服务器）：

```lua
-- ~/.config/nvim/lsp/java_analyzer.lua
return {
  cmd = { "java-analyzer" },
  filetypes = { "java", "yaml", "jproperties" },
  root_dir = function(bufnr, on_dir)
    local name = vim.fs.basename(vim.api.nvim_buf_get_name(bufnr))
    if vim.bo[bufnr].filetype ~= "java"
      and not (name:match("^application") or name:match("^bootstrap")) then
      return
    end
    on_dir(vim.fs.root(bufnr, { "java-analyzer.toml", "settings.gradle", "pom.xml", "build.gradle", ".git" }))
  end,
}
```

Helix：给 `yaml` 和 `properties`（Helix 内置的 Java properties 语言名）加上这个服务器。
Helix 不能按文件名挑选语言服务器，YAML 文件会和原有的 YAML 服务器一起挂上 java-analyzer：

```toml
# ~/.config/helix/languages.toml
[[language]]
name = "yaml"
language-servers = ["yaml-language-server", "java-analyzer"]

[[language]]
name = "properties"
language-servers = ["java-analyzer"]
```

Zed：在 `settings.json` 里给 YAML 和 Java Properties 语言加上这个服务器
（扩展需要在 `language_servers` 里声明这两种语言）：

```json
{
  "languages": {
    "YAML": { "language_servers": ["yaml-language-server", "java-analyzer"] },
    "Java Properties": { "language_servers": ["java-analyzer"] }
  }
}
```

## 修改签名

直接在编辑器里改方法或构造器的声明（增删、重排、改名形参，改形参类型、方法名或返回类型），
然后在声明上打开代码操作，选"Update usages to the new signature"（中文界面为"按新签名更新用法"）：
调用处、覆写链上的其它声明、方法引用和 `@param` 一起改过来。服务器记得的旧签名是这个声明在
第一次被改动之前的样子，中间存盘、格式化、改别处的代码都不影响；改完用法之后客户端会执行
`java-analyzer.acceptSignature` 命令（随代码操作一起下发，各编辑器自动执行），旧签名就此作废。
不能安全完成时（要删掉的实参有副作用、方法引用和新的形参对不上等），支持 `disabled` 的客户端
会把这个操作显示成不可用并给出原因和位置。

## 保存时整理代码

服务器提供两个 source 类代码操作，可以在保存时自动执行：

- `source.organizeImports`：整理 import（删掉没用到的、排序、补上缺少的）；
- `source.fixAll`：一次应用文件里所有不改变行为的修复，包括多余的写法（`int a[]`、多余的
  `toString()` / `new String(...)` / `String.valueOf(...)`）、可以简化的 Stream / 集合调用、
  不必要的装箱拆箱、可以用 `<>` 的类型实参、可以改成模式变量的强制转换、没用到的标签等。
  会改变行为的修复（比如给没抛出的异常补上 `throw`）不会自动应用。

Neovim：

```lua
vim.api.nvim_create_autocmd('BufWritePre', {
  pattern = '*.java',
  callback = function()
    for _, kind in ipairs({ 'source.organizeImports', 'source.fixAll' }) do
      vim.lsp.buf.code_action({ context = { only = { kind } }, apply = true })
    end
  end,
})
```

Zed（`settings.json`）：

```json
{
  "languages": {
    "Java": {
      "code_actions_on_format": { "source.organizeImports": true, "source.fixAll": true }
    }
  }
}
```

## 扩展请求

LSP 之外的请求。服务器在 `initialize` 结果的 `capabilities.experimental` 里以方法名为键
（值为 `true`）声明支持哪些，客户端可以据此探测。

### `java-analyzer/classFile`

按 class 文件的位置或类名找到可以打开的源码文件，用于调试栈帧、异常堆栈里的链接、从 jar
浏览器打开类等只知道 class 文件的场景。参数是下面两种形式之一：

```jsonc
// class 文件所在的容器（路径或 file: URI）和条目名
{ "archive": "/home/u/.m2/.../guava-33.jar", "entry": "com/google/common/collect/ImmutableList.class" }
// 类的二进制名
{ "binaryName": "java.util.ArrayList$Itr" }
```

结果是 `{ "uri": "file:///..." }`，找不到时为 `null`。

- 依赖库和 JDK 的类和跳转定义一样物化成缓存目录下的文件：有源码包（sources jar、JDK 的
  `src.zip`）时解压源码，否则生成桩代码；没有源码包的 jar 会在后台下载 sources jar，下载好后
  桩文件就地换成源码（受 `downloadSources`、`decompiler` 配置控制）。
- 工作区里的类返回它的源文件。`archive` 是某个模块的编译输出目录（`target/classes`、
  `build/classes/java/main` 等）时，多个模块里有同名类的情况下取这个模块的那一份。
- 内部类、匿名类、局部类（`Outer$Inner`、`Outer$1`）对应外层类所在的文件；多版本 jar 的
  `META-INF/versions/N/` 前缀、jmod 的 `classes/` 前缀、JDK 运行时镜像（`lib/modules`）条目
  开头的模块名都会去掉。
- 工程还在导入时，请求等导入完成再回答；可以用 `$/cancelRequest` 取消。

### `java-analyzer/projectClasspath`

模块运行或测试时的类路径和 JDK，用来拼 `java` 命令运行主类或测试。结果和调试器 `launch`
不给 `classPaths` 时算出来的一样。

```jsonc
// 参数
{ "uri": "file:///.../app/src/main/java/com/foo/App.java", "scope": "runtime" }
// 结果
{
  "module": "app",
  "projectRoot": "/abs/app",
  "scope": "runtime",
  "classPaths": ["/abs/app/target/classes", "/abs/core/target/classes", "/home/u/.m2/repository/.../x.jar"],
  "modulePaths": [],
  "outputDirs": ["/abs/app/target/classes"],
  "jdkHome": "/abs/jdk",
  "languageLevel": 17,
  "encoding": "UTF-8"
}
```

- `uri`：源文件，或者模块里的任意路径（模块目录本身也行），按所属的模块算。
- `scope`：`runtime` 或 `test`。不写时看文件在不在测试源码根里。`test` 时测试输出目录排在
  最前，并带上 test scope 的依赖（依赖模块自己的 test scope 依赖不传递）。
- `classPaths`：模块自己的输出目录、它（传递）依赖的模块的输出目录、依赖库的 jar，按这个顺序。
  磁盘上还不存在的输出目录不列出：服务器不负责构建，运行前先编译。
- `outputDirs`：模块自己的编译输出目录，按工程模型给出，不一定已经存在。
- `modulePaths`：目前总是空的。
- `jdkHome`：工程用的 JDK（`jdkHome` 设置、`java-analyzer.toml` 或自动发现的）；找不到时为 `null`。
- `encoding`：模块的源文件编码，按构建文件里写的原样给出：Maven 取编译器插件的 `<encoding>`，
  没有时取（可以继承自父 POM 的）`project.build.sourceEncoding` 属性；Gradle 取
  `compileJava.options.encoding`。没有设置时为 `null`。拼 `java` 命令时可以据此加
  `-Dfile.encoding=<encoding>`，调试器 `launch` 就是这样做的。
- 工程还在导入时，请求等导入完成再回复，可以用 `$/cancelRequest` 取消。路径不属于任何模块时
  返回错误 `RequestFailed`。
- Gradle 工程的依赖取自编译类路径，只在 `runtimeOnly` 里声明的依赖不在其中。

## 调试（Debug Adapter Protocol）

`java-analyzer dap` 在标准输入输出上运行一个调试适配器（DAP，分帧方式和 LSP 一样）。
它直接用 JDWP 协议经 socket 连接目标 JVM，不需要额外的 Java 程序或 jar。

**调试器不负责构建**：启动前先编译好工程（`mvn compile` / `mvn test-compile`、`gradle classes`、
IDE 或脚本里的 `javac -g`）。要看到局部变量，编译时需要带调试信息（Maven 和 Gradle 默认带；
手动用 `javac` 时加 `-g`）。

支持的功能：行断点（类还没加载时先挂起，加载后生效；行上没有代码时挪到下一行代码）、条件断点、
命中次数条件（`5`、`>= 5`、`% 5` 等）、日志断点（`{表达式}` 会被替换成求值结果）、
异常断点（`caught` / `uncaught` 两个过滤器）、继续 / 单步跳过 / 单步进入 / 单步跳出 / 暂停
（单步会跳过 JDK 自己的代码）、线程列表、调用栈（工程源码根下的文件直接打开，依赖库的 `-sources.jar`
和 JDK 的 `src.zip` 里的源码按 `sourceReference` 提供）、局部变量和 `this`、静态字段、对象字段、数组、
字符串和包装类型直接显示值、修改变量（新值可以是表达式），以及表达式求值。

监视、调试控制台、条件断点和日志断点用同一套求值，支持的 Java 表达式：

- 局部变量、`this` 和外层实例的字段（`Outer.this`）、静态字段、字段访问链、数组下标和 `length`、
  `Foo.class`；类型名按源文件的 import、同一个包、`java.lang` 找，没加载的类会让目标 JVM 加载；
- 方法调用：实例方法（按运行时类型分派）、静态方法、`super.m()`、静态 import 的方法；按实参的运行时
  类型选重载，支持装箱拆箱和变长参数；
- `new` 对象和数组（`new int[3]`、`new String[] {"a"}`）；
- 算术、位运算、移位、比较、`&& || !`（短路）、三元、`instanceof`、强制转换；字符串连接时对象调用
  `toString()`，`null` 连成 `"null"`。字符串之间的 `==` 比较内容。

方法在当前停下的线程上执行（只恢复这一个线程），被调用的方法碰到断点不会停。方法抛出异常时求值报错，
带上异常类型和消息；5 秒内没返回的调用会被中止（在被调用的方法里抛出异常），会话照常可用。
只能在因断点、单步或异常停下的线程上调用方法，手动暂停的线程只能读变量和字段。鼠标悬停求值不调用
方法、不新建对象。不支持赋值、自增自减、lambda、方法引用和匿名类。

求值输入框、监视和调试控制台里有补全（`completions` 请求，输入 `.` 时触发）：局部变量和参数、`this`
和外层实例的成员、静态成员、能直接引用的类型和关键字；`值.` 后是值的运行时类型的字段和方法，`类型.`
后是静态成员和成员类，`包.` 后是子包和类型（只列目标 JVM 已经加载的类）。补全和悬停一样不调用方法，
`a.b().` 这样的接收者按方法的返回类型补全。

另外支持把重新编译过的类热替换进运行中的 JVM（见[热替换](#热替换redefineclasses)）。

### launch 参数

| 参数 | 说明 |
| --- | --- |
| `mainClass` | 主类的全限定名（必填）；写成 `模块/主类` 时按模块启动（`-m`） |
| `projectRoot` | 工程根目录，用来导入工程模型、找源码；默认取 `cwd`，再默认取适配器的当前目录 |
| `cwd` | JVM 的工作目录，默认是 `projectRoot` |
| `classPaths` | 类路径（字符串数组）。不给时从工程模型算：主类所在模块的编译输出目录（Maven 的 `target/classes`，主类在测试代码里时加上 `target/test-classes`；Gradle 的 `build/classes/java/main` 等）、它依赖的模块的输出目录和依赖库的 jar |
| `modulePaths` | 模块路径（字符串数组），作为 `--module-path` 传给 JVM |
| `vmArgs` | JVM 参数，字符串（按空白拆分，支持引号）或字符串数组。工程设置了源文件编码（Maven 的 `project.build.sourceEncoding` 或编译器插件的 `<encoding>`，Gradle 的 `compileJava.options.encoding`）而这里没有 `-Dfile.encoding=` 时，自动加上 `-Dfile.encoding=<工程编码>`（取主类所在模块的），免得 JVM 用平台默认编码（例如中文 Windows 上 JDK 17 及以前的 GBK） |
| `args` | 程序参数，字符串或字符串数组 |
| `env` | 环境变量（对象）；值为 `null` 时删掉这个变量 |
| `javaExec` | `java` 可执行文件；默认用工程配置的 JDK、`JAVA_HOME`，或在系统里找到的 JDK |
| `sourcePaths` | 额外的源码目录（字符串数组），显示调用栈时在这些目录里找源文件 |

JVM 的标准输出和标准错误以 `output` 事件转发给编辑器；断开调试（`disconnect`）或 `terminate`
时结束这个 JVM。

### 热替换（`redefineClasses`）

调试过程中改了代码并重新编译后，可以把变化了的类换进正在运行的 JVM，不用重启调试会话。
这是一个自定义请求 `redefineClasses`，`initialize` 的响应里用 `supportsRedefineClasses: true` 声明。

- 不带参数时，检查会话的编译输出目录（launch 时是类路径和模块路径里的目录，attach 时是工程模型里各模块的
  输出目录）里会话开始或上次热替换之后变化了的 class 文件；`outputDirs`（字符串数组）指定别的目录，
  `classFiles`（字符串数组）直接指定要替换的 class 文件。调试器不负责编译，先用构建工具或 IDE 编译。
- 只替换 JVM 已经加载了的类（嵌套类、匿名类各是一个 class 文件，分别处理）；还没加载的类以后加载时
  直接用新版本，只计入结果里的 `notLoaded`。
- 结果 `classes` 里每个类一项：`{ name, file, status: "redefined" | "failed", message? }`。JVM 一般只允许
  修改方法体：增删方法或字段、改父类和接口、改修饰符时替换失败，`message` 说明原因
  （例如“增加了方法；热替换只能修改方法体，需要重启调试会话”），同时以 `output` 事件输出，会话照常继续。
  一批里有替换不了的类时，其余的类照样换上。
- 替换后，已设的断点在新版本的类上重新生效（会发 `breakpoint` 事件更新状态）。正在执行的方法调用
  仍然跑完旧代码，之后的调用走新代码。

VS Code 扩展里是命令 **java-analyzer: Hot Swap Changed Classes**（调试工具栏上也有按钮）；
nvim-dap 里可以用 `require("dap").session():request("redefineClasses", {}, function(err, result) ... end)`。

### attach 参数

先以调试模式启动目标 JVM，例如
`java -agentlib:jdwp=transport=dt_socket,server=y,suspend=y,address=*:5005 -cp ... Main`。

| 参数 | 说明 |
| --- | --- |
| `hostName` | 目标 JVM 所在的主机，默认 `localhost` |
| `port` | JDWP 端口（必填；数字或字符串） |
| `projectRoot` | 工程根目录，用来找源码；默认取适配器的当前目录 |
| `sourcePaths` | 额外的源码目录 |
| `timeout` | 连接超时（毫秒），默认 10000 |

断开时默认让目标 JVM 继续运行（`disconnect` 带 `terminateDebuggee: true` 时结束它）。

### Neovim（nvim-dap）

```lua
local dap = require("dap")

dap.adapters.java = {
  type = "executable",
  command = "java-analyzer",
  args = { "dap" },
}

dap.configurations.java = {
  {
    type = "java",
    request = "launch",
    name = "Launch main class",
    mainClass = function()
      return vim.fn.input("Main class: ", "com.example.Main")
    end,
    projectRoot = "${workspaceFolder}",
    -- classPaths = { "${workspaceFolder}/target/classes" },
    -- vmArgs = "-Xmx512m",
    -- args = { "--verbose" },
  },
  {
    type = "java",
    request = "attach",
    name = "Attach to port 5005",
    hostName = "127.0.0.1",
    port = 5005,
    projectRoot = "${workspaceFolder}",
  },
}
```

### Helix

```toml
# ~/.config/helix/languages.toml
[[language]]
name = "java"
language-servers = ["java-analyzer"]

[language.debugger]
name = "java-analyzer"
transport = "stdio"
command = "java-analyzer"
args = ["dap"]

[[language.debugger.templates]]
name = "launch"
request = "launch"
completion = [{ name = "main class", completion = "_" }]
args = { mainClass = "{0}", projectRoot = "." }

[[language.debugger.templates]]
name = "attach"
request = "attach"
completion = [{ name = "port", completion = "_", default = "5005" }]
args = { hostName = "127.0.0.1", port = "{0}", projectRoot = "." }
```

在 Helix 里用 `:debug-start launch`（或空格 `g` 打开调试菜单）开始调试。

### Zed

Zed 的调试适配器同样由扩展注册：在扩展的 `get_dap_binary` 里返回命令 `java-analyzer`、参数 `["dap"]`，
然后在工程的 `.zed/debug.json` 里写调试配置：

```json
[
  {
    "label": "Launch Main",
    "adapter": "java-analyzer",
    "request": "launch",
    "mainClass": "com.example.Main",
    "projectRoot": "$ZED_WORKTREE_ROOT"
  }
]
```

## 排查问题

- 日志写到标准错误，用环境变量 `JA_LOG` 控制，例如 `JA_LOG=info` 或 `JA_LOG=java_analyzer=debug`。
  调试适配器也一样（`JA_LOG=dap=debug`），启动 JVM 的完整命令行在 `info` 级别输出。
- `java-analyzer analysis-stats <工程目录>` 不经过编辑器直接导入并分析工程，
  会打印识别到的构建系统、JDK、依赖个数和导入过程中的问题。
- 标准库类型全部无法解析时，多半是没有找到 JDK：设置 `JAVA_HOME`，或配置 `jdkHome`。
