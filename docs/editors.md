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

## 排查问题

- 日志写到标准错误，用环境变量 `JA_LOG` 控制，例如 `JA_LOG=info` 或 `JA_LOG=java_analyzer=debug`。
- `java-analyzer analysis-stats <工程目录>` 不经过编辑器直接导入并分析工程，
  会打印识别到的构建系统、JDK、依赖个数和导入过程中的问题。
- 标准库类型全部无法解析时，多半是没有找到 JDK：设置 `JAVA_HOME`，或配置 `jdkHome`。
