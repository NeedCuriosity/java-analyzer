//! 仓库维护任务。用法：`cargo xtask <任务>`。
//!
//! - `codegen [--check]`：根据 `crates/syntax/ast.def` 生成 AST 节点声明
//!   （`crates/syntax/src/ast/generated.rs`）；`--check` 只比较不写入，供 CI 使用；
//! - `dist`：以 release 模式构建并把可执行文件收集到 `dist/`；
//! - `corpus <目录>`：用构建好的二进制对一个源码目录做全量解析（语料测试）；
//! - `ci`：依次执行 fmt 检查、clippy、测试，与 CI 流水线一致。
//!
//! `SyntaxKind` 本身（`crates/parser/src/syntax_kind.rs`）是手写维护的；`codegen` 会校验
//! `ast.def` 里引用的每个种类都在其中定义过。

mod codegen;

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context};

fn project_root() -> PathBuf {
    // xtask 的清单在 <根>/xtask/Cargo.toml
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn cargo() -> Command {
    let mut command = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
    command.current_dir(project_root());
    command
}

fn run(command: &mut Command) -> anyhow::Result<()> {
    let status = command
        .status()
        .with_context(|| format!("无法执行 {command:?}"))?;
    if !status.success() {
        bail!("{command:?} 失败: {status}");
    }
    Ok(())
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "java-analyzer.exe"
    } else {
        "java-analyzer"
    }
}

fn dist() -> anyhow::Result<PathBuf> {
    run(cargo().args(["build", "--release", "--package", "java-analyzer"]))?;
    let root = project_root();
    let built = root.join("target").join("release").join(binary_name());
    let dist = root.join("dist");
    fs::create_dir_all(&dist)?;
    let target = dist.join(binary_name());
    fs::copy(&built, &target)
        .with_context(|| format!("复制 {} 到 {}", built.display(), target.display()))?;
    println!("已生成 {}", target.display());
    Ok(target)
}

fn corpus(dir: &str) -> anyhow::Result<()> {
    let binary = dist()?;
    run(Command::new(binary).args(["parse", dir]))
}

fn ci() -> anyhow::Result<()> {
    codegen::run(&project_root(), true)?;
    run(cargo().args(["fmt", "--all", "--check"]))?;
    run(cargo().args([
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ]))?;
    run(cargo().args(["test", "--workspace"]))
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("codegen") => {
            let check = args.iter().any(|arg| arg == "--check");
            codegen::run(&project_root(), check)
        }
        Some("dist") => dist().map(|_| ()),
        Some("corpus") => match args.get(1) {
            Some(dir) => corpus(dir),
            None => bail!("用法: cargo xtask corpus <源码目录>"),
        },
        Some("ci") => ci(),
        _ => {
            eprintln!("用法: cargo xtask <codegen [--check] | dist | corpus <目录> | ci>");
            Ok(())
        }
    }
}
