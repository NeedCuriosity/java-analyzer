# 评测

`ja-eval` 用两种互补的方式检查分析器的正确性，数据都在这个目录下：

| 部分 | 数据 | 标准答案 | 回答的问题 |
|---|---|---|---|
| 场景评测 | `scenarios/<类别>/*.java` | 写在场景文件里的预期 | 每个 IDE 功能在具体代码上的结果对不对 |
| javac 差分 | `javac/*.java` | 真实的 `javac` | 诊断有没有误报，javac 报的错能检出多少 |
| 格式化语料检查 | 任意一批 `.java` 文件 | 不变量 | 格式化有没有改变含义、有没有把块排歪 |
| 工程检查 | 一个能编译的真实工程 | 工程自己的构建 | 业务工程上有没有误报 |

前两部分是 `ja-eval` 不带子命令时跑的内容，都跑在真实的 JDK 上（不是单元测试用的最小桩），需要 `JAVA_HOME` 指向一个带
`bin/javac` 的 JDK（基线是在 JDK 25 上生成的）。

## 运行

```sh
cargo build --release -p evals
./target/release/ja-eval                    # 全部
./target/release/ja-eval scenarios          # 只跑场景
./target/release/ja-eval javac              # 只跑 javac 差分
./target/release/ja-eval --filter rename    # 名字里含 rename 的场景 / 语料
./target/release/ja-eval --verbose          # 把通过的场景也列出来
./target/release/ja-eval --json out.json    # 完整结果写成 JSON
./target/release/ja-eval --update-baseline  # 用本次结果覆盖 baseline.json
```

退出码非零的条件：

- 有场景失败（标了 `xfail` 的除外）；
- javac 接受的代码被分析器报了错误级诊断（误报）；
- 语料文件本身不能通过 javac；
- 相对 `baseline.json` 退步：场景通过数变少，或者某个类别的检出率下降。

## 场景

一个场景是一个 `.java` 文件：Java 代码里嵌着位置标记和预期，交给分析器之前都会被去掉。

```java
//- /app/Account.java                       多文件场景的分隔行；没有时整个场景是一个文件
package app;

public class Account {
    private long /*@balance*/balance;       // /*@名字*/ 标记一个位置
    public long get() { return /*@read*/balance; }
}
//? def read -> balance                      //? 开头的行是一条预期
//? refs balance = read
//? diag none
```

| 指令 | 含义 |
|---|---|
| `def m -> a` / `-> lib java.util.List#add` / `-> none` | 跳转到定义：落在标记上 / 落在依赖库的类（成员）上 / 没有结果 |
| `typedef m -> ...` | 跳转到类型定义 |
| `impl m -> a b` | 跳转到实现，恰好是这些位置 |
| `refs m = a b` | 查找引用（不含声明），恰好是这些位置 |
| `hover m ~ "文本"` | 悬停内容包含这段文本（`var` 的推断类型也用它检查） |
| `complete m has a b` / `lacks a b` / `first a` | 补全候选里有 / 没有这些标签，或排在第一位的是它 |
| `complete m order a b c` | 这些候选都在，并且按这个先后顺序出现（中间可以隔着别的） |
| `insert m 标签 ~ "文本" [snippet\|plain]` | 选中该候选时插入的文本包含这段文本；可以再要求它是 / 不是带占位符的片段 |
| `stub 类名 ; has "…" ; lacks "…"` | 依赖库里的类生成的签名桩：没有语法错误、里面用到的类型都能解析，并且文本满足条件 |
| `stub 类名 ; refs "…" = a b` | 把签名桩作为打开的库源码文件，在这段文本第一次出现的位置查引用，场景文件里的引用恰好是这些位置 |
| `sig m ~ "文本" active N` | 签名帮助的当前签名和活动形参 |
| `diag none` | 整个场景没有错误级诊断 |
| `diag 代码 m` | `m` 处有一条该代码的诊断（任何级别） |
| `diag exact` | 错误级诊断只有 `diag 代码 m` 列出的那些 |
| `diag only 代码` | 这个代码的诊断（任何级别）只出现在 `diag 代码 m` 列出的位置 |
| `rename m 新名 ; has "…" ; lacks "…"` / `; fails` | 重命名之后的文本满足条件 / 重命名被拒绝 |
| `assist 标识 m [m2] ; has "…" ; lacks "…"` | 在 `m`（或 `m..m2` 选区）应用代码操作之后的文本满足条件 |
| `assist 标识 m ; fails` | 这里提供该代码操作，但它不可用（附带原因） |
| `noassist 标识 m [m2]` | 这里不提供该代码操作 |
| `hint m "标签"` | `m` 处有这条 inlay hint |
| `symbol "查询" "名字" m` | 工作区符号搜索的结果里有这个名字的符号，位置在 `m` |
| `xfail 原因` | 已知做不到：失败记为“预期失败”，通过了会提醒去掉标记 |

分隔行可以带属性，用来描述单个文件之外的东西：

```java
//- /org/testing/Checks.java lib:test-support scope:test    依赖库里的文件：用 javac 编译后打成 jar
package org.testing;                                         挂到类路径上（不带源码）；scope 默认 compile
public class Checks {}
//- /app/Service.java                                        主源码根下的文件（默认）
//- /app/ServiceTest.java root:test                          测试源码根下的文件
//- /app/Service.java formatted                              不是输入：前面同路径的文件格式化之后
                                                             应当得到的全文（一条隐含的预期）
//- /app/Service.java before                                 不是输入：同路径的文件被编辑之前的全文；
                                                             两段之间的差异当作编辑器里的一次修改，
                                                             "按新签名更新用法"（change_signature）
                                                             由此得知旧签名
//- /org/core/Repo.java lib:core open                        依赖库的源码，同时作为"打开的库源码文件"
                                                             交给分析器（像跳进依赖库后打开的文件），
                                                             里面可以有标记
//- /java/lang/Class.java open                               不带 lib：类路径上已有的类（JDK）的源码，
                                                             可以只写一部分成员
```

- 有 `lib:` 或 `root:test` 的场景按"一个模块、主 / 测试两个源码根"的工程来装，
  用来检查 scope 隔离、没有源码的库（签名桩、形参名）这类单文件表达不了的行为。
- 格式化场景写成"输入 + `formatted` 段"。已经排好的代码把同一段文本写两遍，
  表示格式化不应当改动它。

写场景时的约定：

- 正例和反例放在一起。诊断类场景用 `diag exact` 收尾，同一个文件里既有该报的写法，
  也有长得像但合法的写法（`ok1`、`ok2` …），两头都卡住。
- 一个场景只测一个主题，名字说明主题；类别就是所在的子目录。
- 暴露出分析器缺陷的场景，先修分析器；确实做不到的才标 `xfail` 并写明原因。
- `xfail` 作用于整个场景文件。一个主题里只有部分预期失败时，把失败的那几条拆到单独的
  场景里再标 `xfail`，其余的继续把关。

## javac 差分

`javac/` 下是一批自包含、能通过编译的文件。对每个文件：

1. 原样交给 javac 和分析器。javac 接受的代码，分析器报任何错误级诊断都算**误报**。
2. 用变异算子在语法树上制造单点错误，每个变体再交给两边。变体错没错、错在哪一行、
   属于哪一类，全部以 javac 的输出为准，不需要人工标注。

| 算子 | 做什么 | 主要对应的 javac 错误 |
|---|---|---|
| `undefined-name` / `undefined-method` / `undefined-type` | 把变量名 / 方法名 / 类型名改成未定义的名字 | `cant.resolve` |
| `wrong-type` | 把初值换成类型不符的字面量 | `prob.found.req` |
| `to-void` | 把方法的返回类型改成 `void` | `prob.found.req`、`override.incompatible.ret` |
| `nonbool-condition` | 把 `if` 的条件换成整数 | `prob.found.req` |
| `bad-operand` | 把算术 / 比较运算的右操作数换成 `true` | `operator.cant.be.applied` |
| `extra-arg` | 给方法调用多加一个实参 | `cant.apply.symbol` |
| `duplicate-local` | 在局部变量声明后面再声明一个同名变量 | `already.defined` |
| `make-final` | 给局部变量加上 `final` | `cant.assign.val.to.var` |
| `make-static` | 给实例方法加上 `static` | `non-static.cant.be.ref`、`override.static` |
| `narrow-access` | 把方法的 `public` 改成 `private` | `override.weaker.access` |
| `drop-override` | 删掉带 `@Override` 的方法 | `does.not.override.abstract` |
| `drop-case` | 删掉 switch 表达式的最后一个分支 | `not.exhaustive` |
| `drop-return` | 删掉方法末尾的 `return` | `missing.ret.stmt` |
| `dead-code` | 在 `return` / `throw` 后面加一条语句 | `unreachable.stmt` |
| `drop-initializer` | 删掉局部变量的初值 | `var.might.not.have.been.initialized` |
| `drop-throws` | 删掉方法的 `throws` 子句 | `unreported.exception` |

报告里的几个数：

- **误报**：必须是 0。
- **检出率**：javac 报错的行上分析器也报了的比例，按 javac 的错误类别和按算子各统计一次。
  分析器的诊断是保守的（拿不准就不报），检出率不要求 100%，用基线防止退步。
- **多报**：变体里分析器报在了 javac 没报的行上。javac 在类型检查出错之后不再做流分析，
  所以这种变体里分析器的可达性 / 确定赋值 / 异常诊断不计入。
- **没有造成错误的变体**：变异之后 javac 仍然接受。这时分析器也不能报错，否则算误报。

语料里没有"已知失败"的机制：javac 接受而分析器会误报的写法先放进场景并标 `xfail`，
修好之后再挪进语料。

加语料：往 `javac/` 里放一个能单独编译的文件（`public` 类名和文件名一致，只依赖 JDK），
覆盖面越广越好；然后 `--update-baseline`。

## 格式化语料检查

```sh
./target/release/ja-eval format-check <目录> [--max-misaligned N]
```

对目录下的每个 `.java` 文件做格式化，要求：

- 格式化前后非空白 token 的序列逐个相同（注释只比较去掉每行首尾空白后的内容）、不多出语法错误、
  对结果再格式化一遍没有任何改动。这几条说的是"没有改变代码的含义"，必须是 0。
- 花括号没有被排歪：独占行首的 `}` 要和它所属的声明 / 语句的起始行缩进相同。原文满足、
  格式化之后不满足的文件单独计数，超过 `--max-misaligned`（默认 0）算失败。这一条和具体的
  代码风格无关，查的是"折行之后块的缩进算错"这一类问题。

不需要 JDK。前一组性质管不了排版对不对（缩进全错也能满足），具体的排版结果由 `format/`
下的场景检查。

## 工程检查

```sh
./target/release/ja-eval project <工程目录> [--max-errors N] [--allow-incomplete]
```

导入一个真实的工程（Maven / Gradle / 手动配置），对全部源文件跑诊断：一个能通过编译的工程上，
错误级诊断应当是 0，出现的每一条都是误报。场景和 javac 语料都是只依赖 JDK 的小文件，
多模块、第三方依赖、Lombok、框架注解这些业务工程里才有的东西靠这一项覆盖。

- 工程能不能编译以它自己的构建为准：先用构建工具编译一遍（顺带把依赖下载到本地仓库）。
- 导入时有任何问题（依赖缺失、父 POM 找不到）默认直接算失败——缺了依赖之后"没有报错"
  多半只是保守策略没敢报。确实要在不完整的工程上看结果时加 `--allow-incomplete`。
- `--max-errors N` 给已知的误报留一个上限，只防止变多。

## 协议层

场景直接调用 `ide` 层的接口，看不到 LSP 协议转换里的行为。这部分由
`crates/java-analyzer/src/tests.rs` 里的端到端测试覆盖（进程内启动服务器，走完整的
initialize → didOpen → 请求流程）：补全项的 `textEdit` / `insertTextFormat` / `sortText`、
界面文案是否跟随 `initialize` 的 `locale`、code lens 的开关。已知做不到的用
`#[ignore = "已知问题：…"]` 标出来，`cargo test -p java-analyzer -- --ignored` 可以看到它们的状态。

## 基线

`baseline.json` 记录场景通过数和各类别的检出率。评测有进步时用 `--update-baseline`
更新并提交；退步会让 `ja-eval` 以非零退出码结束，CI 的 `evals` 任务靠它把关。
