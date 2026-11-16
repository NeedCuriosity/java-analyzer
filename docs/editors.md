# 编辑器接入

java-analyzer 只说标准 LSP，通过标准输入输出通信。任何支持 LSP 的编辑器都可以接入：
把 `java-analyzer` 放到 `PATH` 里，让编辑器在打开 `.java` 文件时启动它即可。

配置项通过 `initializationOptions` 传入（见 [configuration.md](configuration.md)）。

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
本仓库不包含编辑器扩展；需要在 Zed 里使用时，写一个最小的扩展，在
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

## 排查问题

- 日志写到标准错误，用环境变量 `JA_LOG` 控制，例如 `JA_LOG=info` 或 `JA_LOG=java_analyzer=debug`。
- `java-analyzer analysis-stats <工程目录>` 不经过编辑器直接导入并分析工程，
  会打印识别到的构建系统、JDK、依赖个数和导入过程中的问题。
- 标准库类型全部无法解析时，多半是没有找到 JDK：设置 `JAVA_HOME`，或配置 `jdkHome`。
