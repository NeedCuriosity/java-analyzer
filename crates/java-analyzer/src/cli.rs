//! 命令行子命令：不经过 LSP，直接对磁盘上的代码做解析 / 分析并输出统计。

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use base_db::SourceRootKind;
use ide::{AnalysisHost, Change, DiagnosticCode, DiagnosticsConfig, FileChange, FileKind};
use rayon::prelude::*;
use vfs::Vfs;

fn java_files(path: &Path) -> Vec<vfs::LoadedFile> {
    if path.is_file() {
        return vfs::read_file_lossy(path)
            .map(|text| vfs::LoadedFile {
                path: path.to_path_buf(),
                text,
            })
            .into_iter()
            .collect();
    }
    vfs::load_java_files(&[path.to_path_buf()])
}

/// `java-analyzer parse <路径>`
pub fn parse(path: &Path) -> anyhow::Result<()> {
    let start = Instant::now();
    let files = java_files(path);
    let load_time = start.elapsed();
    anyhow::ensure!(
        !files.is_empty(),
        base_db::tr!(
            "{} 下没有 .java 文件",
            "No .java files under {}",
            path.display()
        )
    );

    let start = Instant::now();
    // 每个文件：(错误列表, 往返是否无损)
    let results: Vec<(PathBuf, Vec<String>, bool)> = files
        .par_iter()
        .map(|file| {
            let parse = syntax::ast::CompilationUnit::parse(&file.text);
            let lossless = parse.syntax_node().text() == file.text.as_str();
            let index = ide::LineIndex::new(&file.text);
            let errors = parse
                .errors()
                .iter()
                .map(|err| {
                    let pos = index.line_col(err.range.start());
                    format!(
                        "{}:{}:{}: {}",
                        file.path.display(),
                        pos.line + 1,
                        pos.col + 1,
                        err.message
                    )
                })
                .collect();
            (file.path.clone(), errors, lossless)
        })
        .collect();
    let parse_time = start.elapsed();

    let mut error_count = 0;
    let mut files_with_errors = 0;
    let mut lossy = 0;
    for (path, errors, lossless) in &results {
        if !errors.is_empty() {
            files_with_errors += 1;
        }
        error_count += errors.len();
        for error in errors.iter().take(20) {
            println!("{error}");
        }
        if !lossless {
            lossy += 1;
            println!(
                "{}",
                base_db::tr!(
                    "{}: 语法树文本与原文不一致",
                    "{}: syntax tree text differs from the source",
                    path.display()
                )
            );
        }
    }
    let bytes: usize = files.iter().map(|f| f.text.len()).sum();
    let lines: usize = files.iter().map(|f| f.text.lines().count()).sum();
    println!();
    let mb = bytes as f64 / 1e6;
    let count = files.len();
    println!(
        "{}",
        base_db::tr!(
            "文件: {count}（{lines} 行, {mb:.1} MB）",
            "Files: {count} ({lines} lines, {mb:.1} MB)"
        )
    );
    println!(
        "{}",
        base_db::tr!(
            "读取: {load_time:.2?}  解析: {parse_time:.2?}",
            "Load: {load_time:.2?}  Parse: {parse_time:.2?}"
        )
    );
    println!(
        "{}",
        base_db::tr!(
            "语法错误: {error_count}（分布在 {files_with_errors} 个文件）",
            "Syntax errors: {error_count} (in {files_with_errors} files)"
        )
    );
    println!(
        "{}",
        base_db::tr!("往返不一致: {lossy}", "Lossy round trips: {lossy}")
    );
    anyhow::ensure!(
        lossy == 0,
        base_db::tr!(
            "存在语法树与原文不一致的文件",
            "Some syntax trees differ from their source text"
        )
    );
    Ok(())
}

/// `java-analyzer analysis-stats <工程路径>`
pub fn analysis_stats(root: &Path) -> anyhow::Result<()> {
    let root = root.canonicalize()?;

    let start = Instant::now();
    let project = project_model::load_project(&root, &project_model::LoadOptions::default());
    println!(
        "{}",
        base_db::tr!(
            "工程: {:?}, {} 个模块, {} 个依赖, JDK: {}（{:.2?}）",
            "Project: {:?}, {} modules, {} libraries, JDK: {} ({:.2?})",
            project.kind,
            project.graph.modules.len(),
            project.graph.libraries.len(),
            project.graph.jdk.as_ref().map_or_else(
                || base_db::trs!("未找到", "not found").to_owned(),
                |jdk| format!("{} ({})", jdk.version, jdk.home.display())
            ),
            start.elapsed(),
        )
    );
    for message in &project.messages {
        println!("  {}{message}", base_db::trs!("注意: ", "Note: "));
    }

    let start = Instant::now();
    let class_path = project_model::build_class_path(&project.graph);
    println!(
        "{}",
        base_db::tr!(
            "类路径: {} 个类（{:.2?}）",
            "Class path: {} classes ({:.2?})",
            class_path.len(),
            start.elapsed()
        )
    );

    let start = Instant::now();
    let files = vfs::load_java_files(&project.graph.all_source_roots());
    let lines: usize = files.iter().map(|f| f.text.lines().count()).sum();
    println!(
        "{}",
        base_db::tr!(
            "源码: {} 个文件, {} 行（{:.2?}）",
            "Sources: {} files, {} lines ({:.2?})",
            files.len(),
            lines,
            start.elapsed()
        )
    );

    let graph = Arc::new(project.graph);
    let mut vfs = Vfs::default();
    let mut change = Change {
        project: Some(graph.clone()),
        class_path: Some(Arc::new(class_path)),
        ..Change::default()
    };
    // Spring 配置文件：归到模块根是其最长前缀的那个模块
    let module_roots: Vec<PathBuf> = graph.modules.iter().map(|m| m.root.clone()).collect();
    let resources: Vec<base_db::ResourceFile> = vfs::load_spring_configs(&module_roots)
        .into_iter()
        .chain(vfs::load_lombok_configs(&module_roots))
        .map(|file| {
            let module = graph
                .modules
                .iter()
                .enumerate()
                .filter(|(_, m)| file.path.starts_with(&m.root))
                .max_by_key(|(_, m)| m.root.components().count())
                .map(|(i, _)| base_db::ModuleId(i as u32));
            base_db::ResourceFile {
                file_id: vfs.set_disk_contents(&file.path, Some(file.text.clone())),
                name: file
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into())
                    .unwrap_or_default(),
                module,
                text: Arc::from(file.text),
            }
        })
        .collect();
    change.resources = Some(Arc::new(resources));
    let mut file_ids = Vec::with_capacity(files.len());
    for file in files {
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
        let file_id = vfs.set_disk_contents(&file.path, Some(file.text.clone()));
        change.files.push((
            file_id,
            Some(FileChange {
                name: base_db::file_stem(&file.path.to_string_lossy()),
                text: Arc::from(file.text),
                kind,
            }),
        ));
        file_ids.push(file_id);
    }
    let mut host = AnalysisHost::new();
    host.apply_change(change);
    let analysis = host.analysis();

    let start = Instant::now();
    let (_, symbols) = analysis.prime_caches()?;
    println!(
        "{}",
        base_db::tr!(
            "索引: {symbols} 个符号（{:.2?}）",
            "Index: {symbols} symbols ({:.2?})",
            start.elapsed()
        )
    );

    // 全量诊断：等价于对每个方法体做一遍类型推断
    let start = Instant::now();
    let config = DiagnosticsConfig::default();
    let per_file: Vec<(usize, usize, usize, usize, std::time::Duration)> = file_ids
        .par_iter()
        .map_with(analysis.clone(), |analysis, file_id| {
            let file_start = Instant::now();
            let diagnostics = analysis.diagnostics(*file_id, &config).unwrap_or_default();
            let (known, total) = analysis.expr_type_stats(*file_id).unwrap_or((0, 0));
            let syntax_errors = diagnostics
                .iter()
                .filter(|d| d.code == DiagnosticCode::SyntaxError)
                .count();
            // 只数错误：提示和警告（未使用的符号、可疑的写法）在合法代码里本来就会有
            let semantic = diagnostics
                .iter()
                .filter(|d| {
                    d.severity == ide::Severity::Error && d.code != DiagnosticCode::SyntaxError
                })
                .count();
            (syntax_errors, semantic, known, total, file_start.elapsed())
        })
        .collect();

    // 语义诊断按种类汇总；设置 JA_STATS_SAMPLES=N 时每类再列出前 N 条，方便排查误报
    let samples: usize = std::env::var("JA_STATS_SAMPLES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut by_code: std::collections::BTreeMap<&'static str, (usize, Vec<String>)> =
        Default::default();
    for file_id in &file_ids {
        for diagnostic in analysis.diagnostics(*file_id, &config).unwrap_or_default() {
            if matches!(
                diagnostic.code,
                DiagnosticCode::SyntaxError | DiagnosticCode::UnusedImport
            ) {
                continue;
            }
            let entry = by_code.entry(diagnostic.code.as_str()).or_default();
            entry.0 += 1;
            if entry.1.len() < samples {
                let path = vfs.file_path(*file_id).display().to_string();
                let line = std::fs::read_to_string(&path)
                    .map(|text| {
                        let offset = usize::from(diagnostic.range.start()).min(text.len());
                        text[..offset].matches('\n').count() + 1
                    })
                    .unwrap_or(0);
                entry
                    .1
                    .push(format!("{path}:{line}: {}", diagnostic.message));
            }
        }
    }
    let infer_time = start.elapsed();

    let syntax_errors: usize = per_file.iter().map(|r| r.0).sum();
    let semantic: usize = per_file.iter().map(|r| r.1).sum();
    let known: usize = per_file.iter().map(|r| r.2).sum();
    let total: usize = per_file.iter().map(|r| r.3).sum();
    let mut durations: Vec<_> = per_file.iter().map(|r| r.4).collect();
    durations.sort();
    let percentile = |p: f64| {
        durations
            .get(((durations.len() as f64 * p) as usize).min(durations.len().saturating_sub(1)))
            .copied()
            .unwrap_or_default()
    };

    println!(
        "{}",
        base_db::tr!(
            "推断: 全部文件（{infer_time:.2?}，墙钟时间，多线程）",
            "Inference: all files ({infer_time:.2?} wall clock, multi-threaded)"
        )
    );
    println!(
        "{}",
        base_db::tr!(
            "  单文件耗时 p50 {:.2?}  p95 {:.2?}  最大 {:.2?}",
            "  Per file p50 {:.2?}  p95 {:.2?}  max {:.2?}",
            percentile(0.50),
            percentile(0.95),
            percentile(1.0),
        )
    );
    println!(
        "{}",
        base_db::tr!(
            "语法错误: {syntax_errors}",
            "Syntax errors: {syntax_errors}"
        )
    );
    println!(
        "{}",
        base_db::tr!(
            "语义错误: {semantic}（合法工程上应当接近 0，否则多半是误报）；各类诊断的数量:",
            "Semantic errors: {semantic} (should be close to 0 on a valid project; otherwise they are likely false positives); diagnostics by code:"
        )
    );
    for (code, (count, examples)) in &by_code {
        println!("  {code}: {count}");
        for example in examples {
            println!("    {example}");
        }
    }
    if std::env::var_os("JA_DIAG_TIMING").is_some() {
        println!(
            "{}",
            base_db::trs!("各项检查的累计耗时:", "Total time per check:")
        );
        for (name, elapsed) in ide::diagnostics_timing::totals() {
            println!("  {name}: {elapsed:.2?}");
        }
    }
    // 代码操作在下面的延迟抽样里才运行，它们的耗时在抽样之后单独列出
    let before_sampling = ide::diagnostics_timing::totals();
    if total > 0 {
        let rate = known as f64 * 100.0 / total as f64;
        println!(
            "{}",
            base_db::tr!(
                "表达式类型已知率: {rate:.1}%（{known} / {total}）",
                "Known expression types: {rate:.1}% ({known} / {total})"
            )
        );
    }

    request_latencies(&analysis, &file_ids)?;
    if std::env::var_os("JA_DIAG_TIMING").is_some() {
        println!(
            "{}",
            base_db::trs!("各代码操作的累计耗时:", "Total time per assist:")
        );
        for (name, elapsed) in ide::diagnostics_timing::totals() {
            let earlier = before_sampling
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, d)| *d);
            let spent = elapsed.saturating_sub(earlier.unwrap_or_default());
            if name.starts_with("assist::") && !spent.is_zero() {
                println!("  {name}: {spent:.2?}");
            }
        }
    }

    match peak_memory_mb() {
        Some(mb) => println!(
            "{}",
            base_db::tr!("内存峰值: {mb:.0} MB", "Peak memory: {mb:.0} MB")
        ),
        None => println!(
            "{}",
            base_db::trs!(
                "内存峰值: 此平台上无法获取",
                "Peak memory: not available on this platform"
            )
        ),
    }
    Ok(())
}

/// 抽样测量交互式请求的延迟：在成员访问 `x.name` 的 `name` 处做悬停、跳转和补全，
/// 在这些位置和成员声明（`private` / `public` 之后）上取代码操作（编辑器每次移动光标都会问）。
/// 此时各级缓存都是热的，对应的是"打开工程一段时间后"的体验。
fn request_latencies(analysis: &ide::Analysis, file_ids: &[ide::FileId]) -> anyhow::Result<()> {
    const MAX_FILES: usize = 200;
    const SAMPLES_PER_FILE: usize = 5;

    let step = (file_ids.len() / MAX_FILES).max(1);
    let completion_config = ide::CompletionConfig::default();
    let mut hover = Vec::new();
    let mut definition = Vec::new();
    let mut completion = Vec::new();
    let mut assists = Vec::new();

    for file_id in file_ids.iter().step_by(step).take(MAX_FILES) {
        let Some(text) = analysis.file_text(*file_id)? else {
            continue;
        };
        // `.` 后面紧跟标识符的位置
        let candidates: Vec<usize> = text
            .match_indices('.')
            .map(|(idx, _)| idx + 1)
            .filter(|idx| text[*idx..].starts_with(|c: char| c.is_alphabetic()))
            .collect();
        let stride = (candidates.len() / SAMPLES_PER_FILE).max(1);
        for offset in candidates.iter().step_by(stride).take(SAMPLES_PER_FILE) {
            let position = ide::FilePosition {
                file_id: *file_id,
                offset: syntax::TextSize::from(*offset as u32),
            };

            let start = Instant::now();
            let _ = analysis.hover(position)?;
            hover.push(start.elapsed());

            let start = Instant::now();
            let _ = analysis.goto_definition(position)?;
            definition.push(start.elapsed());

            let start = Instant::now();
            let _ = analysis.completions(position, &completion_config)?;
            completion.push(start.elapsed());
        }
        let declarations: Vec<usize> = text
            .match_indices("private ")
            .chain(text.match_indices("public "))
            .map(|(idx, keyword)| idx + keyword.len())
            .collect();
        let offsets = candidates
            .iter()
            .step_by(stride)
            .take(SAMPLES_PER_FILE)
            .chain(
                declarations
                    .iter()
                    .step_by((declarations.len() / SAMPLES_PER_FILE).max(1))
                    .take(SAMPLES_PER_FILE),
            );
        for offset in offsets {
            let offset = syntax::TextSize::from(*offset as u32);
            let start = Instant::now();
            let _ = analysis.assists(ide::FileRange {
                file_id: *file_id,
                range: syntax::TextRange::empty(offset),
            })?;
            assists.push(start.elapsed());
        }
    }

    let report = |name: &str, samples: &mut Vec<std::time::Duration>| {
        if samples.is_empty() {
            return;
        }
        samples.sort();
        let at = |p: f64| samples[((samples.len() as f64 * p) as usize).min(samples.len() - 1)];
        println!(
            "{}",
            base_db::tr!(
                "  {:<6} p50 {:>9.2?}  p95 {:>9.2?}  最大 {:>9.2?}（{} 次）",
                "  {:<10} p50 {:>9.2?}  p95 {:>9.2?}  max {:>9.2?} ({} samples)",
                name,
                at(0.50),
                at(0.95),
                at(1.0),
                samples.len(),
            )
        );
    };
    println!(
        "{}",
        base_db::trs!(
            "请求延迟（抽样，缓存已热）:",
            "Request latency (sampled, warm caches):"
        )
    );
    report(base_db::trs!("补全", "completion"), &mut completion);
    report(base_db::trs!("悬停", "hover"), &mut hover);
    report(base_db::trs!("跳转", "definition"), &mut definition);
    report(base_db::trs!("代码操作", "assists"), &mut assists);
    Ok(())
}

/// 进程的常驻内存峰值。只在 Linux 上可用（读 `/proc/self/status` 的 `VmHWM`）。
fn peak_memory_mb() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0)
}
