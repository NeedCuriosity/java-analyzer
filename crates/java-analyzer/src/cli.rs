//! 命令行子命令：不经过 LSP，直接对磁盘上的代码做解析 / 分析并输出统计。

mod completion_stats;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use base_db::SourceRootKind;
use ide::{AnalysisHost, Change, DiagnosticCode, DiagnosticsConfig, FileChange, FileKind};
use rayon::prelude::*;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind, SyntaxNode, TextRange, TextSize,
};
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

/// `java-analyzer analysis-stats <工程路径>...`：多个路径按多根工作区合并导入。
pub fn analysis_stats(roots: &[PathBuf]) -> anyhow::Result<()> {
    let roots = roots
        .iter()
        .map(|root| root.canonicalize())
        .collect::<Result<Vec<_>, _>>()?;

    let start = Instant::now();
    let project = project_model::load_projects(&roots, &project_model::LoadOptions::default());
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
        .chain(vfs::load_dir_configs(
            &module_roots,
            &graph.all_source_roots(),
        ))
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
                dir: base_db::file_dir(&file.path.to_string_lossy()),
                module,
                text: Arc::from(file.text),
            }
        })
        .collect();
    change.resources = Some(Arc::new(resources));
    let mut file_ids = Vec::with_capacity(files.len());
    let mut file_kinds = Vec::with_capacity(files.len());
    // 编辑后延迟的抽样要原样写回文件，记下每个文件的名字和角色
    let mut file_meta: Vec<(ide::FileId, FileChange)> = Vec::with_capacity(files.len());
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
        let file_change =
            FileChange::at_path(&file.path.to_string_lossy(), Arc::from(file.text), kind);
        file_meta.push((file_id, file_change.clone()));
        change.files.push((file_id, Some(file_change)));
        file_ids.push(file_id);
        file_kinds.push((file_id, kind));
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

    // JA_DIAG_VERIFY：逐个文件核对按体缓存得到的诊断和整个文件一起检查的结果是否相同
    if std::env::var_os("JA_DIAG_VERIFY").is_some() {
        let mismatched: usize = file_ids
            .par_iter()
            .map_with(analysis.clone(), |analysis, file_id| {
                let split = analysis.diagnostics(*file_id, &config).unwrap_or_default();
                let whole = analysis
                    .whole_file_diagnostics(*file_id, &config)
                    .unwrap_or_default();
                if split == whole {
                    return 0;
                }
                let path = vfs.file_path(*file_id).display().to_string();
                let mut report =
                    base_db::tr!("诊断不一致: {path}\n", "Diagnostics differ: {path}\n");
                for d in split.iter().filter(|d| !whole.contains(d)) {
                    report += &format!("  + {:?} {:?} {}\n", d.range, d.code, d.message);
                }
                for d in whole.iter().filter(|d| !split.contains(d)) {
                    report += &format!("  - {:?} {:?} {}\n", d.range, d.code, d.message);
                }
                print!("{report}");
                1
            })
            .sum();
        println!(
            "{}",
            base_db::tr!(
                "诊断核对: {mismatched} 个文件不一致",
                "Diagnostics check: {mismatched} files differ"
            )
        );
    }

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

    // JA_STATS_UNKNOWN=N：列出前 N 个类型未知的表达式（只取最内层的，外层往往只是被它连累）
    let unknown_samples: usize = std::env::var("JA_STATS_UNKNOWN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if unknown_samples > 0 {
        let mut shown = 0;
        'files: for file_id in &file_ids {
            let ranges = analysis.unknown_expr_ranges(*file_id)?;
            if ranges.is_empty() {
                continue;
            }
            let Some(text) = analysis.file_text(*file_id)? else {
                continue;
            };
            let path = vfs.file_path(*file_id).display().to_string();
            for range in &ranges {
                // 包含别的未知表达式的跳过
                if ranges
                    .iter()
                    .any(|r| r != range && range.contains_range(*r))
                {
                    continue;
                }
                let start = usize::from(range.start());
                let line = text[..start].matches('\n').count() + 1;
                let snippet: String = text[range.start().into()..range.end().into()]
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .chars()
                    .take(100)
                    .collect();
                println!("  ? {path}:{line}: {snippet}");
                shown += 1;
                if shown >= unknown_samples {
                    break 'files;
                }
            }
        }
    }
    request_latencies(&analysis, &file_ids)?;
    // 命中率要改文件文本：先放掉快照，否则写入会等它
    drop(analysis);
    completion_stats::completion_hit_rate(&mut host, &vfs, &file_kinds)?;
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
    // 编辑要写数据库（快照在上面已经放掉）
    edit_latencies(&mut host, &file_meta)?;

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
    // 查找引用和重命名要扫整个工作区，每个文件只取一个位置
    let mut references = Vec::new();
    let mut renames = Vec::new();

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
        if let Some(offset) = candidates.get(candidates.len() / 2) {
            let position = ide::FilePosition {
                file_id: *file_id,
                offset: syntax::TextSize::from(*offset as u32),
            };
            let start = Instant::now();
            let _ = analysis.find_all_refs(position, true)?;
            references.push(start.elapsed());

            let start = Instant::now();
            let _ = analysis.rename(position, "renamedForStats")?;
            renames.push(start.elapsed());
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

    let report = report_latency;
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
    report(base_db::trs!("查找引用", "references"), &mut references);
    report(base_db::trs!("重命名", "rename"), &mut renames);
    Ok(())
}

/// 反编译用的类环境：直接读一个容器。
struct ContainerEnv {
    container: Box<dyn classfile::ClassContainer>,
    names: rustc_hash::FxHashSet<String>,
}

impl decompiler::ClassEnv for ContainerEnv {
    fn class_bytes(&self, binary_name: &str) -> Option<Vec<u8>> {
        self.container.read_class(binary_name)
    }

    fn class_exists(&self, binary_name: &str) -> bool {
        self.names.contains(binary_name)
    }
}

/// `java-analyzer decompile <jar 或 JDK 的 lib/modules> [类名或包前缀]`
///
/// 给了完整类名时输出这个类的反编译结果；否则反编译（前缀下的）全部顶层类，
/// 报告成功率、失败原因、语法错误数和耗时。
pub fn decompile(path: &Path, filter: Option<&str>) -> anyhow::Result<()> {
    let container: Box<dyn classfile::ClassContainer> =
        if path.file_name().is_some_and(|n| n == "modules") {
            Box::new(
                classfile::JImageContainer::open(path.to_path_buf()).ok_or_else(|| {
                    anyhow::anyhow!(base_db::tr!(
                        "无法读取 {}",
                        "Cannot read {}",
                        path.display()
                    ))
                })?,
            )
        } else {
            Box::new(classfile::JarContainer::new(path.to_path_buf()))
        };
    let all = container.class_names();
    let env = ContainerEnv {
        names: all.iter().cloned().collect(),
        container,
    };
    let filter = filter.map(|f| f.replace('.', "/"));
    if let Some(class) = filter.as_deref().filter(|c| env.names.contains(*c)) {
        let bytes = env.container.read_class(class).unwrap_or_default();
        let mut stats = decompiler::DecompileStats::default();
        let start = Instant::now();
        let text = decompiler::decompile_class_text(&bytes, &env, &mut stats)?;
        let elapsed = start.elapsed();
        print!("{text}");
        let errors = syntax::ast::CompilationUnit::parse(&text).errors().len();
        eprintln!(
            "{}",
            base_db::tr!(
                "{} 个方法，反编译成功 {}，语法错误 {}，用时 {:.2?}",
                "{} methods, {} decompiled, {} syntax errors, {:.2?}",
                stats.methods,
                stats.decompiled,
                errors,
                elapsed
            )
        );
        return Ok(());
    }
    let prefix = filter.unwrap_or_default();
    let top: Vec<&String> = all
        .iter()
        .filter(|n| !n.contains('$') && n.starts_with(&prefix) && !n.ends_with("module-info"))
        .collect();
    let start = Instant::now();
    let results: Vec<(decompiler::DecompileStats, usize, std::time::Duration)> = top
        .par_iter()
        .map(|name| {
            let mut stats = decompiler::DecompileStats::default();
            let Some(bytes) = env.container.read_class(name) else {
                return (stats, 0, std::time::Duration::ZERO);
            };
            let t = Instant::now();
            let Ok(text) = decompiler::decompile_class_text(&bytes, &env, &mut stats) else {
                return (stats, 0, t.elapsed());
            };
            let elapsed = t.elapsed();
            let errors = syntax::ast::CompilationUnit::parse(&text).errors().len();
            if errors > 0 {
                eprintln!("{name}: {errors}");
            }
            (stats, errors, elapsed)
        })
        .collect();
    let wall = start.elapsed();
    let mut total = decompiler::DecompileStats::default();
    let mut errors = 0;
    let mut slowest = std::time::Duration::ZERO;
    let mut per_class: Vec<std::time::Duration> = Vec::new();
    for (s, e, d) in &results {
        total.merge(s);
        errors += e;
        slowest = slowest.max(*d);
        per_class.push(*d);
    }
    per_class.sort();
    let median = per_class
        .get(per_class.len() / 2)
        .copied()
        .unwrap_or_default();
    println!(
        "{}",
        base_db::tr!(
            "{} 个类，{} 个方法，反编译成功 {} ({:.2}%)，回退 {}，语法错误 {}",
            "{} classes, {} methods, {} decompiled ({:.2}%), {} fell back, {} syntax errors",
            total.classes,
            total.methods,
            total.decompiled,
            100.0 * total.decompiled as f64 / total.methods.max(1) as f64,
            total.methods - total.decompiled,
            errors
        )
    );
    println!(
        "{}",
        base_db::tr!(
            "用时 {:.2?}（每个顶层类中位数 {:.2?}，最慢 {:.2?}）",
            "time {:.2?} (median per top-level class {:.2?}, slowest {:.2?})",
            wall,
            median,
            slowest
        )
    );
    let mut failures: Vec<_> = total.failures.iter().collect();
    failures.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    for (reason, n) in failures {
        println!("  {n:>8}  {reason}");
    }
    anyhow::ensure!(
        errors == 0,
        base_db::trs!(
            "反编译结果有语法错误",
            "decompiled output has syntax errors"
        )
    );
    Ok(())
}

/// 打印一组耗时的 p50 / p95 / 最大值。
fn report_latency(name: &str, samples: &mut [Duration]) {
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
}

/// 编辑后延迟抽样里模拟的一种编辑。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EditKind {
    /// 在方法体里的标识符末尾敲一个字符
    InsertChar,
    /// 删掉方法体里某个标识符的最后一个字符
    DeleteChar,
    /// 在方法体开头插入一条新语句
    NewStatement,
    /// 改局部变量声明处的名字
    RenameLocal,
    /// 给方法加一个形参
    MethodSignature,
    /// 改字段的类型
    FieldType,
    /// 在类里新增一个方法
    NewMethod,
    /// 加一条 import
    AddImport,
}

impl EditKind {
    const ALL: [EditKind; 8] = [
        EditKind::InsertChar,
        EditKind::DeleteChar,
        EditKind::NewStatement,
        EditKind::RenameLocal,
        EditKind::MethodSignature,
        EditKind::FieldType,
        EditKind::NewMethod,
        EditKind::AddImport,
    ];

    fn label(self) -> &'static str {
        match self {
            EditKind::InsertChar => base_db::trs!("方法体内插入字符", "insert char in body"),
            EditKind::DeleteChar => base_db::trs!("方法体内删除字符", "delete char in body"),
            EditKind::NewStatement => base_db::trs!("插入新语句", "insert statement"),
            EditKind::RenameLocal => base_db::trs!("改局部变量名", "rename local"),
            EditKind::MethodSignature => base_db::trs!("改方法签名", "change method signature"),
            EditKind::FieldType => base_db::trs!("改字段类型", "change field type"),
            EditKind::NewMethod => base_db::trs!("新增方法", "add method"),
            EditKind::AddImport => base_db::trs!("加 import", "add import"),
        }
    }

    /// 编辑只落在一个方法体里
    fn in_body(self) -> bool {
        matches!(
            self,
            EditKind::InsertChar
                | EditKind::DeleteChar
                | EditKind::NewStatement
                | EditKind::RenameLocal
        )
    }
}

/// 一处模拟的编辑：把 `range` 替换成 `text`，编辑后光标停在 `cursor`。
struct PlannedEdit {
    range: TextRange,
    text: String,
    cursor: TextSize,
}

impl PlannedEdit {
    fn apply(&self, original: &str) -> String {
        let mut res = original.to_owned();
        res.replace_range(
            usize::from(self.range.start())..usize::from(self.range.end()),
            &self.text,
        );
        res
    }
}

/// 抽样用的方法：有方法体、体里有局部变量声明和名字引用的方法里居中的那个。
fn sample_method(root: &SyntaxNode) -> Option<ast::MethodDecl> {
    let methods: Vec<ast::MethodDecl> = root
        .descendants()
        .filter_map(ast::MethodDecl::cast)
        .filter(|m| {
            m.body().is_some_and(|body| {
                let body = body.syntax();
                body.descendants()
                    .any(|n| n.kind() == SyntaxKind::LOCAL_VAR_DECL)
                    && body.descendants().any(|n| n.kind() == SyntaxKind::NAME_REF)
                    // 不要局部类 / 匿名类里的方法：编辑它们的体会改变类的列表
                    && !m
                        .syntax()
                        .ancestors()
                        .skip(1)
                        .any(|a| matches!(a.kind(), SyntaxKind::BLOCK | SyntaxKind::NEW_EXPR))
            })
        })
        .collect();
    methods.get(methods.len() / 2).cloned()
}

/// 在文件里给一种编辑找位置。找不到合适的位置时返回 `None`。
fn plan_edit(text: &str, kind: EditKind) -> Option<PlannedEdit> {
    let root = ast::CompilationUnit::parse(text).syntax_node();
    let method = sample_method(&root)?;
    let body = method.body()?;
    // 方法体里第一个多于一个字符的名字引用
    let name_ref = || {
        body.syntax()
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| {
                t.kind() == SyntaxKind::IDENT
                    && t.text().chars().count() > 1
                    && t.parent().is_some_and(|p| p.kind() == SyntaxKind::NAME_REF)
                    // 不在局部类 / 匿名类里：那里的编辑可能改变类的签名
                    && !t
                        .parent_ancestors()
                        .take_while(|a| a != body.syntax())
                        .any(|a| a.kind() == SyntaxKind::CLASS_BODY)
            })
    };
    let insert = |offset: TextSize, text: &str, cursor: TextSize| PlannedEdit {
        range: TextRange::empty(offset),
        text: text.to_owned(),
        cursor,
    };
    Some(match kind {
        EditKind::InsertChar => {
            let end = name_ref()?.text_range().end();
            insert(end, "x", end + TextSize::from(1))
        }
        EditKind::DeleteChar => {
            let token = name_ref()?;
            let end = token.text_range().end();
            // 删掉最后一个字符（标识符可以是中文等多字节字符）
            let last = token.text().chars().next_back()?;
            let range = TextRange::new(end - TextSize::of(last), end);
            PlannedEdit {
                range,
                text: String::new(),
                cursor: range.start(),
            }
        }
        EditKind::NewStatement => {
            let l_curly = body.syntax().first_token()?.text_range().end();
            let stmt = "\n        int jaTmp = 0;";
            // 光标停在 `= ` 之后：在这里要补全表达式
            let cursor = l_curly + TextSize::of(&stmt[..stmt.len() - 2]);
            insert(l_curly, stmt, cursor)
        }
        EditKind::RenameLocal => {
            let name = body
                .syntax()
                .descendants()
                .filter_map(ast::LocalVarDecl::cast)
                .find_map(|decl| decl.declarators().next()?.name())?;
            let end = name.syntax().text_range().end();
            insert(end, "Renamed", end + TextSize::of("Renamed"))
        }
        EditKind::MethodSignature => {
            let params = method.param_list()?;
            let r_paren = params
                .syntax()
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| t.kind() == SyntaxKind::R_PAREN)?
                .text_range()
                .start();
            let added = if params.params().next().is_some() {
                ", int jaExtra"
            } else {
                "int jaExtra"
            };
            insert(r_paren, added, r_paren + TextSize::of(added))
        }
        EditKind::FieldType => {
            let ty = root
                .descendants()
                .filter_map(ast::FieldDecl::cast)
                .find_map(|field| field.ty())?;
            let range = ty.syntax().text_range();
            let new = if ty.syntax().text() == "Object" {
                "String"
            } else {
                "Object"
            };
            PlannedEdit {
                range,
                text: new.to_owned(),
                cursor: range.start() + TextSize::of(new),
            }
        }
        EditKind::NewMethod => {
            let class_body = method.syntax().parent()?;
            let r_curly = class_body.last_token()?;
            if r_curly.kind() != SyntaxKind::R_CURLY {
                return None;
            }
            let at = r_curly.text_range().start();
            let added = "\n    void jaNewMethod() {  }\n";
            // 光标在新方法的花括号之间
            insert(at, added, at + TextSize::of("\n    void jaNewMethod() { "))
        }
        EditKind::AddImport => {
            let added = "\nimport java.util.concurrent.atomic.AtomicLong;";
            match root
                .children()
                .find(|n| n.kind() == SyntaxKind::PACKAGE_DECL)
            {
                Some(package) => {
                    let end = package.text_range().end();
                    insert(end, added, end + TextSize::of(added))
                }
                None => {
                    let added = &added[1..];
                    insert(0.into(), &format!("{added}\n"), TextSize::of(added))
                }
            }
        }
    })
}

/// 依赖 `text` 这个文件的另一个文件：用到了它的顶层类（同包里直接写类名，或者 import 了它）。
fn find_dependent(
    files: &[(ide::FileId, FileChange)],
    file_id: ide::FileId,
    text: &str,
) -> Option<ide::FileId> {
    let root = ast::CompilationUnit::parse(text).tree();
    let class = root.type_decls().next()?.name()?.text();
    let package = root
        .package()
        .and_then(|p| p.name())
        .map(|n| n.segments().join("."))
        .unwrap_or_default();
    let same_package = format!("package {package};");
    let qualified = format!("{package}.{class}");
    let star = format!("import {package}.*;");
    let mentions = |text: &str| {
        text.match_indices(class.as_str()).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = text[at + class.len()..].chars().next();
            let ident = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            !ident(before) && !ident(after)
        })
    };
    files
        .iter()
        .filter(|(id, _)| *id != file_id)
        .find(|(_, change)| {
            let other: &str = &change.text;
            mentions(other)
                && (other.contains(&qualified)
                    || other.contains(&star)
                    || (!package.is_empty() && other.contains(&same_package)))
        })
        .map(|(id, _)| *id)
}

/// 光标前后各 40 行：编辑器里大致可见的范围。
/// `cursor` 可能是编辑后文本里的位置，用在原文本上时会落在多字节字符中间，先退到字符边界。
fn visible_range(text: &str, cursor: TextSize) -> TextRange {
    let mut cursor = usize::from(cursor).min(text.len());
    while !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let start = text[..cursor]
        .rmatch_indices('\n')
        .nth(40)
        .map_or(0, |(i, _)| i + 1);
    let end = text[cursor..]
        .match_indices('\n')
        .nth(40)
        .map_or(text.len(), |(i, _)| cursor + i);
    TextRange::new((start as u32).into(), (end as u32).into())
}

/// 编辑后的延迟：在抽样文件里模拟各种编辑，每次编辑后单独测一个请求（之前的状态是原文本上
/// 各请求都算过一遍），然后恢复原文本。另外统计编辑后重新执行的推断查询个数，衡量失效范围。
///
/// 抽样文件数由 `JA_EDIT_FILES` 设置（默认 30，另加最大的 3 个文件）。设置 `JA_EDIT_QUERIES=1`
/// 时按编辑种类列出编辑后重新执行的全部查询。
fn edit_latencies(
    host: &mut AnalysisHost,
    files: &[(ide::FileId, FileChange)],
) -> anyhow::Result<()> {
    let sample_count: usize = std::env::var("JA_EDIT_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    if sample_count == 0 || files.is_empty() {
        return Ok(());
    }
    let show_queries = std::env::var_os("JA_EDIT_QUERIES").is_some();
    // 均匀抽样，再加上最大的几个文件（大文件最考验增量）
    let mut by_size: Vec<usize> = (0..files.len()).collect();
    by_size.sort_by_key(|&i| std::cmp::Reverse(files[i].1.text.len()));
    // 不是每个文件都有合适的位置：多取一些候选
    let step = (files.len() / (sample_count * 2)).max(1);
    let largest: Vec<usize> = by_size.iter().take(3).copied().collect();
    let picked: Vec<usize> = largest
        .iter()
        .copied()
        .chain(
            // 先隔一个取一个，覆盖整个文件列表；不够再取剩下的
            (0..files.len())
                .step_by(step * 2)
                .chain((step..files.len()).step_by(step * 2))
                .filter(|i| !largest.contains(i)),
        )
        .collect();

    let config = DiagnosticsConfig::default();
    let completion_config = ide::CompletionConfig::default();

    #[derive(Default)]
    struct Samples {
        diagnostics: Vec<Duration>,
        completion: Vec<Duration>,
        highlight: Vec<Duration>,
        inlay: Vec<Duration>,
        dependent: Vec<Duration>,
        /// 当前文件的诊断让多少个体重新推断 / 文件里共有多少个体
        reinferred: Vec<(usize, usize)>,
        /// 依赖文件的诊断让多少个体重新推断 / 那个文件里共有多少个体
        dependent_reinferred: Vec<(usize, usize)>,
        queries: rustc_hash::FxHashMap<String, usize>,
        /// 编辑后当前文件的诊断里各项检查的累计耗时（`JA_DIAG_TIMING`）
        checks: Vec<(&'static str, Duration)>,
        /// 各个请求核对后复用的缓存个数（`JA_EDIT_QUERIES`）
        validated: [Vec<usize>; 5],
    }
    let mut samples: Vec<Samples> = EditKind::ALL.iter().map(|_| Samples::default()).collect();
    let mut sampled_files = 0;

    let set_text = |host: &mut AnalysisHost, idx: usize, text: Arc<str>| {
        let (file_id, change) = &files[idx];
        host.apply_change(Change {
            files: vec![(
                *file_id,
                Some(FileChange {
                    text,
                    ..change.clone()
                }),
            )],
            ..Change::default()
        });
    };

    let start = Instant::now();
    for idx in picked {
        if sampled_files >= sample_count + 3 {
            break;
        }
        let (file_id, original) = (&files[idx].0, &files[idx].1);
        let file_id = *file_id;
        let original_text = original.text.clone();
        let plans: Vec<Option<PlannedEdit>> = EditKind::ALL
            .iter()
            .map(|kind| plan_edit(&original_text, *kind))
            .collect();
        // 至少要能做方法体内的编辑
        if plans[0].is_none() {
            continue;
        }
        sampled_files += 1;
        let dependent = find_dependent(files, file_id, &original_text);

        for (kind_idx, (kind, plan)) in EditKind::ALL.iter().zip(&plans).enumerate() {
            let Some(plan) = plan else { continue };
            let edited: Arc<str> = Arc::from(plan.apply(&original_text));
            let visible = visible_range(&edited, plan.cursor);
            let position = ide::FilePosition {
                file_id,
                offset: plan.cursor,
            };
            // 原文本上把各个请求都算一遍：编辑发生在"文件已经分析过"的状态上
            let warm = |host: &AnalysisHost| -> anyhow::Result<()> {
                let analysis = host.analysis();
                analysis.diagnostics(file_id, &config)?;
                analysis.highlight(file_id, None)?;
                analysis.inlay_hints(file_id, Some(visible_range(&original_text, plan.cursor)))?;
                analysis.completions(
                    ide::FilePosition {
                        file_id,
                        offset: plan.range.start(),
                    },
                    &completion_config,
                )?;
                if let Some(dep) = dependent {
                    analysis.diagnostics(dep, &config)?;
                }
                Ok(())
            };
            let out = &mut samples[kind_idx];
            type Request<'a> = Box<dyn Fn(&ide::Analysis) -> anyhow::Result<()> + 'a>;
            let mut requests: Vec<(&mut Vec<Duration>, Request)> = vec![
                (
                    &mut out.diagnostics,
                    Box::new(|a: &ide::Analysis| {
                        a.diagnostics(file_id, &config)?;
                        Ok(())
                    }),
                ),
                (
                    &mut out.completion,
                    Box::new(|a: &ide::Analysis| {
                        a.completions(position, &completion_config)?;
                        Ok(())
                    }),
                ),
                (
                    &mut out.highlight,
                    Box::new(|a: &ide::Analysis| {
                        a.highlight(file_id, None)?;
                        Ok(())
                    }),
                ),
                (
                    &mut out.inlay,
                    Box::new(|a: &ide::Analysis| {
                        a.inlay_hints(file_id, Some(visible))?;
                        Ok(())
                    }),
                ),
            ];
            if let Some(dep) = dependent {
                requests.push((
                    &mut out.dependent,
                    Box::new(move |a: &ide::Analysis| {
                        a.diagnostics(dep, &config)?;
                        Ok(())
                    }),
                ));
            }
            let mut checks_before = Vec::new();
            let mut checks_after = Vec::new();
            let mut validated_counts = Vec::new();
            for (i, (durations, request)) in requests.iter_mut().enumerate() {
                warm(&*host)?;
                set_text(&mut *host, idx, edited.clone());
                let analysis = host.analysis();
                if i == 0 {
                    checks_before = ide::diagnostics_timing::totals();
                }
                let start = Instant::now();
                if show_queries {
                    // 每个请求核对了多少缓存：编辑之后第一次查询的固定开销
                    let (res, _, validated) = analysis.record_query_events(|a| request(a));
                    res?;
                    let total: usize = validated.0.values().sum();
                    validated_counts.push((i, total));
                } else {
                    request(&analysis)?;
                }
                durations.push(start.elapsed());
                if i == 0 {
                    checks_after = ide::diagnostics_timing::totals();
                }
                drop(analysis);
                set_text(&mut *host, idx, original_text.clone());
            }
            drop(requests);
            for (i, total) in validated_counts {
                out.validated[i].push(total);
            }
            for (name, after) in checks_after {
                let before = checks_before
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map_or(Duration::ZERO, |(_, d)| *d);
                let spent = after.saturating_sub(before);
                match out.checks.iter_mut().find(|(n, _)| *n == name) {
                    Some((_, total)) => *total += spent,
                    None => out.checks.push((name, spent)),
                }
            }

            // 失效范围：编辑后当前文件和依赖文件的诊断各让多少个体重新推断
            warm(&*host)?;
            set_text(&mut *host, idx, edited.clone());
            let analysis = host.analysis();
            let (bodies, executions) = analysis.record_executions(|a| {
                a.diagnostics(file_id, &config)?;
                a.body_count(file_id)
            });
            out.reinferred
                .push((executions.count(INFER_QUERY), bodies?));
            let mut all = executions.0;
            if let Some(dep) = dependent {
                let (bodies, executions) = analysis.record_executions(|a| {
                    a.diagnostics(dep, &config)?;
                    a.body_count(dep)
                });
                out.dependent_reinferred
                    .push((executions.count(INFER_QUERY), bodies?));
                // 方法体内的编辑不应让依赖文件的诊断重算：列出例外，方便排查
                if show_queries && kind.in_body() && executions.count("default_diagnostics") > 0 {
                    let dep_name = files
                        .iter()
                        .find(|(id, _)| *id == dep)
                        .map(|(_, change)| change.name.clone())
                        .unwrap_or_default();
                    println!(
                        "    {} {} -> {dep_name}: {:?}",
                        base_db::trs!("依赖文件的诊断被重算:", "Dependent diagnostics recomputed:"),
                        original.name,
                        executions.0
                    );
                }
                for (name, count) in executions.0 {
                    *all.entry(name).or_default() += count;
                }
            }
            for (name, count) in all {
                *out.queries.entry(name).or_default() += count;
            }
            drop(analysis);
            set_text(&mut *host, idx, original_text.clone());
        }
    }

    println!(
        "{}",
        base_db::tr!(
            "编辑后延迟（{sampled_files} 个文件，每次编辑后单独测一个请求，{:.2?}）:",
            "Latency after edits ({sampled_files} files, one request measured per edit, {:.2?}):",
            start.elapsed()
        )
    );
    for (kind, out) in EditKind::ALL.iter().zip(&mut samples) {
        if out.diagnostics.is_empty() {
            continue;
        }
        println!("  {}:", kind.label());
        report_latency(
            base_db::trs!("  诊断", "  diagnostics"),
            &mut out.diagnostics,
        );
        report_latency(base_db::trs!("  补全", "  completion"), &mut out.completion);
        report_latency(base_db::trs!("  高亮", "  highlight"), &mut out.highlight);
        report_latency(base_db::trs!("  内嵌提示", "  inlay hints"), &mut out.inlay);
        report_latency(
            base_db::trs!("  依赖文件诊断", "  dependent diagnostics"),
            &mut out.dependent,
        );
        let sum = |v: &[(usize, usize)]| v.iter().fold((0, 0), |(a, b), (x, y)| (a + x, b + y));
        let (re, total) = sum(&out.reinferred);
        let (dep_re, dep_total) = sum(&out.dependent_reinferred);
        println!(
            "{}",
            base_db::tr!(
                "    重新推断的体: 当前文件 {re} / {total}（{} 次编辑），依赖文件 {dep_re} / {dep_total}（{} 次编辑）",
                "    Re-inferred bodies: edited file {re} / {total} ({} edits), dependent file {dep_re} / {dep_total} ({} edits)",
                out.reinferred.len(),
                out.dependent_reinferred.len(),
            )
        );
        if kind.in_body() {
            // 理想情况：只有被编辑的那个体重新推断
            let excess = re.saturating_sub(out.reinferred.len());
            println!(
                "{}",
                base_db::tr!(
                    "    方法体内编辑后多推断的其它体: 当前文件 {excess}，依赖文件 {dep_re}",
                    "    Other bodies re-inferred after an in-body edit: edited file {excess}, dependent file {dep_re}",
                )
            );
        }
        if !out.checks.is_empty() {
            out.checks.sort_by_key(|(_, d)| std::cmp::Reverse(*d));
            println!(
                "{}",
                base_db::trs!(
                    "    当前文件诊断里各项检查的累计耗时:",
                    "    Time per check in the edited file's diagnostics:"
                )
            );
            for (name, spent) in out.checks.iter().take(12) {
                println!("      {name}: {spent:.2?}");
            }
        }
        if show_queries {
            let names = [
                "diagnostics",
                "completion",
                "highlight",
                "inlay",
                "dependent",
            ];
            let validated: Vec<String> = names
                .iter()
                .zip(&out.validated)
                .filter(|(_, v)| !v.is_empty())
                .map(|(name, v)| format!("{name} {}", v.iter().sum::<usize>() / v.len()))
                .collect();
            println!(
                "{}",
                base_db::tr!(
                    "    每个请求平均核对的缓存数: {}",
                    "    Memos validated per request (mean): {}",
                    validated.join(", ")
                )
            );
            let mut queries: Vec<_> = out.queries.iter().collect();
            queries.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            for (name, count) in queries.iter().take(25) {
                println!("      {name}: {count}");
            }
        }
    }
    Ok(())
}

/// 方法体推断查询的名字（统计重新执行次数用）。
const INFER_QUERY: &str = "infer_tracked";

/// 进程的常驻内存峰值。只在 Linux 上可用（读 `/proc/self/status` 的 `VmHWM`）。
fn peak_memory_mb() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 标识符和注释里有中文时，模拟编辑和可见范围都不能切在字符中间。
    #[test]
    fn edit_plans_respect_multibyte_text() {
        let text = "class 缓存 {\n    int 读取(int 数量) {\n        int 结果 = 数量; // 缓存结果\n        return 结果;\n    }\n}\n";
        for kind in EditKind::ALL {
            let Some(plan) = plan_edit(text, kind) else {
                continue;
            };
            let edited = plan.apply(text);
            visible_range(&edited, plan.cursor);
            visible_range(text, plan.cursor);
        }
        let range = visible_range("// 缓", TextSize::from(4));
        assert_eq!(range, TextRange::new(0.into(), 6.into()));
    }
}
