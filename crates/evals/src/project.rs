//! 工程级检查：在一个真实的、能通过编译的工程上，分析器不应当报任何错误级诊断。
//!
//! 场景和 javac 语料都是只依赖 JDK 的小文件；业务工程里才有的东西（多模块、第三方依赖、
//! Lombok、框架注解）要靠这一项来覆盖。工程能不能编译以它自己的构建为准，
//! 所以要先用构建工具编译一遍（顺带把依赖下载到本地仓库），再运行这项检查。

use std::{collections::BTreeMap, path::Path, sync::Arc, time::Instant};

use base_db::SourceRootKind;
use ide::{AnalysisHost, Change, DiagnosticsConfig, FileChange, FileId, FileKind, Severity};
use rayon::prelude::*;

/// `allow_incomplete`：导入时有问题（依赖缺失等）也照样检查。默认不允许，
/// 因为缺了依赖之后"没有报错"多半只是保守策略没敢报，说明不了什么。
pub fn run(root: &Path, max_errors: usize, allow_incomplete: bool) -> anyhow::Result<bool> {
    let root = root.canonicalize()?;
    let started = Instant::now();
    let project = project_model::load_project(&root, &project_model::LoadOptions::default());
    println!(
        "== 工程检查 {}（{:?}，{} 个模块，{} 个依赖）==",
        root.display(),
        project.kind,
        project.graph.modules.len(),
        project.graph.libraries.len()
    );
    for message in &project.messages {
        println!("  注意: {message}");
    }
    if !project.messages.is_empty() && !allow_incomplete {
        println!("  工程没有完整导入：先用构建工具把工程编译一遍，或者加 --allow-incomplete");
        return Ok(false);
    }
    let missing: Vec<&str> = project
        .graph
        .libraries
        .iter()
        .filter(|lib| !lib.jar.is_file())
        .map(|lib| lib.name.as_str())
        .collect();
    if !missing.is_empty() {
        println!(
            "  有 {} 个依赖的 jar 不在本地（例如 {}）：先用构建工具把工程编译一遍",
            missing.len(),
            missing[0]
        );
        return Ok(false);
    }

    let class_path = project_model::build_class_path(&project.graph);
    let files = vfs::load_java_files(&project.graph.all_source_roots());
    let graph = Arc::new(project.graph);
    let mut change = Change {
        project: Some(graph.clone()),
        class_path: Some(Arc::new(class_path)),
        ..Change::default()
    };
    for (idx, file) in files.iter().enumerate() {
        let kind = match graph.source_root_of(&file.path) {
            Some((module, source_root)) => FileKind::Workspace {
                module: Some(module),
                root: source_root.kind,
            },
            None => FileKind::Workspace {
                module: None,
                root: SourceRootKind::Main,
            },
        };
        change.files.push((
            FileId(idx as u32),
            Some(FileChange::at_path(
                &file.path.to_string_lossy(),
                Arc::from(file.text.as_str()),
                kind,
            )),
        ));
    }
    let mut host = AnalysisHost::new();
    host.apply_change(change);
    let analysis = host.analysis();

    let config = DiagnosticsConfig::default();
    let mut errors: Vec<(String, &'static str)> = (0..files.len())
        .into_par_iter()
        .map_with(analysis.clone(), |analysis, idx| {
            let file = &files[idx];
            analysis
                .diagnostics(FileId(idx as u32), &config)
                .unwrap_or_default()
                .into_iter()
                .filter(|d| d.severity == Severity::Error)
                .map(|d| {
                    let offset = usize::from(d.range.start()).min(file.text.len());
                    let line = file.text[..offset].matches('\n').count() + 1;
                    let path = file.path.strip_prefix(&root).unwrap_or(&file.path);
                    (
                        format!(
                            "{}:{line} [{}] {}",
                            path.display(),
                            d.code.as_str(),
                            d.message
                        ),
                        d.code.as_str(),
                    )
                })
                .collect::<Vec<_>>()
        })
        .flatten()
        .collect();
    errors.sort();

    let lines: usize = files.iter().map(|f| f.text.lines().count()).sum();
    println!(
        "  {} 个文件，{lines} 行（{:.2?}）",
        files.len(),
        started.elapsed()
    );
    println!("  错误级诊断: {}（允许 {max_errors}）", errors.len());
    let mut by_code: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, code) in &errors {
        *by_code.entry(code).or_default() += 1;
    }
    for (code, count) in &by_code {
        println!("    {code}: {count}");
    }
    for (message, _) in errors.iter().take(50) {
        println!("      {message}");
    }
    if errors.len() > 50 {
        println!("      ……还有 {} 条", errors.len() - 50);
    }
    Ok(errors.len() <= max_errors)
}
