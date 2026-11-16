//! `ja-eval`：java-analyzer 的评测工具。
//!
//! ```text
//! ja-eval [scenarios|javac|all] [选项]
//!
//!   scenarios   手写的场景评测（evals/scenarios/<类别>/*.java）
//!   javac       以 javac 为标准答案的变异差分评测（evals/javac/*.java）
//!   all         两者都跑（默认）
//!
//!   --dir <目录>          评测数据的根目录，默认 ./evals
//!   --filter <子串>       只跑名字里包含该子串的场景 / 语料文件
//!   --sites <N>           每个文件上每个变异算子最多取几个位置，默认 4
//!   --json <文件>         把完整结果写成 JSON
//!   --update-baseline     把本次结果写进 <目录>/baseline.json
//!   --verbose             列出每个通过的场景
//! ```
//!
//! 另有两个独立的子命令：`ja-eval format-check <目录> [--max-misaligned N]`（格式化语料检查）和
//! `ja-eval project <工程目录> [--max-errors N] [--allow-incomplete]`（真实工程上不应当有
//! 错误级诊断）。
//!
//! 退出码非零的情形：有场景失败（标了 `xfail` 的除外）、javac 接受的代码被报了错、
//! 语料本身不能通过 javac、或者检出数低于基线。

mod format_check;
mod javac;
mod project;
mod run;
mod scenario;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::Context;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    run::{Env, ScenarioResult},
    scenario::Scenario,
};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CategoryScore {
    passed: usize,
    total: usize,
    /// 标了 xfail 且确实失败的
    expected_failures: usize,
}

/// 写进 `baseline.json` 的内容：下次运行不能比它差。
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Baseline {
    scenarios: BTreeMap<String, CategoryScore>,
    /// javac 差分里，受支持的类别检出了多少条
    javac_detected: usize,
    javac_errors: usize,
    /// javac 接受的代码里 javac 报了 unchecked 警告的行，分析器检出了多少
    #[serde(default)]
    javac_unchecked_detected: usize,
    #[serde(default)]
    javac_unchecked_warnings: usize,
}

#[derive(Debug, Serialize)]
struct FullReport<'a> {
    jdk: &'a str,
    scenarios: Vec<ScenarioJson<'a>>,
    javac: Option<&'a javac::JavacReport>,
}

#[derive(Debug, Serialize)]
struct ScenarioJson<'a> {
    name: &'a str,
    category: &'a str,
    checks: usize,
    passed: bool,
    xfail: Option<&'a str>,
    failures: &'a [String],
}

struct Options {
    mode: String,
    dir: PathBuf,
    filter: Option<String>,
    sites: usize,
    json: Option<PathBuf>,
    update_baseline: bool,
    verbose: bool,
}

fn parse_args() -> anyhow::Result<Options> {
    let mut options = Options {
        mode: "all".to_owned(),
        dir: PathBuf::from("evals"),
        filter: None,
        sites: 4,
        json: None,
        update_baseline: false,
        verbose: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().with_context(|| format!("{name} 需要一个参数"));
        match arg.as_str() {
            "scenarios" | "javac" | "all" => options.mode = arg,
            "--dir" => options.dir = PathBuf::from(value("--dir")?),
            "--filter" => options.filter = Some(value("--filter")?),
            "--sites" => options.sites = value("--sites")?.parse().context("--sites 应为数字")?,
            "--json" => options.json = Some(PathBuf::from(value("--json")?)),
            "--update-baseline" => options.update_baseline = true,
            "--verbose" => options.verbose = true,
            "--help" | "-h" => {
                println!("用法: ja-eval [scenarios|javac|all] [--dir 目录] [--filter 子串] [--sites N] [--json 文件] [--update-baseline] [--verbose]");
                std::process::exit(0);
            }
            other => anyhow::bail!("不认识的参数 `{other}`（--help 查看用法）"),
        }
    }
    Ok(options)
}

fn collect_scenarios(root: &Path, filter: Option<&str>) -> anyhow::Result<Vec<Scenario>> {
    fn visit(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                visit(&path, out)?;
            } else if path.extension().is_some_and(|ext| ext == "java") {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut paths = Vec::new();
    visit(root, &mut paths).with_context(|| format!("无法读取 {}", root.display()))?;
    paths.sort();

    let mut scenarios = Vec::new();
    for path in paths {
        let relative = path.strip_prefix(root).unwrap_or(&path).with_extension("");
        let name = relative.to_string_lossy().replace('\\', "/");
        if filter.is_some_and(|f| !name.contains(f)) {
            continue;
        }
        let category = name.split('/').next().unwrap_or("misc").to_owned();
        let source = std::fs::read_to_string(&path)?;
        let scenario = scenario::parse_scenario(&name, &category, &source)
            .map_err(|err| anyhow::anyhow!("{}: {err}", path.display()))?;
        scenarios.push(scenario);
    }
    Ok(scenarios)
}

fn score(results: &[ScenarioResult]) -> BTreeMap<String, CategoryScore> {
    let mut scores: BTreeMap<String, CategoryScore> = BTreeMap::new();
    for result in results {
        let entry = scores.entry(result.category.clone()).or_default();
        entry.total += 1;
        if result.passed() {
            entry.passed += 1;
        } else if result.xfail.is_some() {
            entry.expected_failures += 1;
        }
    }
    scores
}

fn percent(part: usize, total: usize) -> String {
    if total == 0 {
        "-".to_owned()
    } else {
        format!("{:.0}%", part as f64 * 100.0 / total as f64)
    }
}

fn main() -> anyhow::Result<()> {
    // `ja-eval format-check <目录>`：格式化器的语料检查，不需要 JDK
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.first().map(String::as_str) == Some("format-check") {
        const USAGE: &str = "用法: ja-eval format-check <目录> [--max-misaligned N]";
        let dir = raw.get(1).context(USAGE)?;
        let max_misaligned = match raw.get(2).map(String::as_str) {
            None => 0,
            Some("--max-misaligned") => raw
                .get(3)
                .and_then(|n| n.parse().ok())
                .context("--max-misaligned 后面应为数字")?,
            Some(_) => anyhow::bail!(USAGE),
        };
        let ok = format_check::run(Path::new(dir), max_misaligned)?;
        std::process::exit(if ok { 0 } else { 1 });
    }
    // `ja-eval project <目录> [--max-errors N]`：真实工程上的错误级诊断
    if raw.first().map(String::as_str) == Some("project") {
        const USAGE: &str =
            "用法: ja-eval project <工程目录> [--max-errors N] [--allow-incomplete]";
        let dir = raw.get(1).context(USAGE)?;
        let mut max_errors = 0;
        let mut allow_incomplete = false;
        let mut rest = raw[2..].iter();
        while let Some(arg) = rest.next() {
            match arg.as_str() {
                "--max-errors" => {
                    max_errors = rest
                        .next()
                        .and_then(|n| n.parse().ok())
                        .context("--max-errors 后面应为数字")?;
                }
                "--allow-incomplete" => allow_incomplete = true,
                _ => anyhow::bail!(USAGE),
            }
        }
        let ok = project::run(Path::new(dir), max_errors, allow_incomplete)?;
        std::process::exit(if ok { 0 } else { 1 });
    }
    let options = parse_args()?;
    let env = Env::new()?;
    println!("JDK: {}", env.jdk);
    let mut problems: Vec<String> = Vec::new();

    // ---- 场景评测 ----
    let mut results: Vec<ScenarioResult> = Vec::new();
    if options.mode != "javac" {
        let start = Instant::now();
        let scenarios =
            collect_scenarios(&options.dir.join("scenarios"), options.filter.as_deref())?;
        results = scenarios
            .par_iter()
            .map(|s| run::run_scenario(&env, s))
            .collect();
        results.sort_by(|a, b| a.name.cmp(&b.name));

        println!(
            "\n== 场景评测（{} 个场景，{:.2?}）==",
            results.len(),
            start.elapsed()
        );
        for result in &results {
            match (result.passed(), &result.xfail) {
                (true, None) => {
                    if options.verbose {
                        println!("  通过      {}（{} 条预期）", result.name, result.checks);
                    }
                }
                (true, Some(reason)) => {
                    println!(
                        "  意外通过  {}（标了 xfail: {reason}）——可以去掉 xfail 了",
                        result.name
                    );
                }
                (false, Some(reason)) => {
                    println!("  预期失败  {}（{reason}）", result.name);
                    if options.verbose {
                        for failure in &result.failures {
                            println!("      {failure}");
                        }
                    }
                }
                (false, None) => {
                    println!("  失败      {}", result.name);
                    for failure in &result.failures {
                        println!("      {}", failure.replace('\n', "\n      "));
                    }
                    problems.push(format!("场景 {} 失败", result.name));
                }
            }
        }
        println!(
            "\n  {:<14} {:>6} {:>6} {:>8} {:>8}",
            "类别", "通过", "总数", "预期失败", "通过率"
        );
        let scores = score(&results);
        for (category, s) in &scores {
            println!(
                "  {:<14} {:>6} {:>6} {:>8} {:>8}",
                category,
                s.passed,
                s.total,
                s.expected_failures,
                percent(s.passed, s.total)
            );
        }
        let passed: usize = scores.values().map(|s| s.passed).sum();
        let total: usize = scores.values().map(|s| s.total).sum();
        let checks: usize = results.iter().map(|r| r.checks).sum();
        println!(
            "  {:<14} {:>6} {:>6} {:>8} {:>8}   （共 {checks} 条预期）",
            "合计",
            passed,
            total,
            "",
            percent(passed, total)
        );
    }

    // ---- javac 差分 ----
    let mut javac_report = None;
    if options.mode != "scenarios" {
        let start = Instant::now();
        let report = javac::run(
            &env,
            &options.dir.join("javac"),
            options.filter.as_deref(),
            options.sites,
        )?;
        println!(
            "\n== javac 差分评测（{} 个语料文件，{} 个变体，{:.2?}）==",
            report.corpus_files,
            report.mutants,
            start.elapsed()
        );
        for broken in &report.broken_corpus {
            println!("  语料不能编译  {broken}");
            problems.push(format!("语料文件不能通过 javac: {broken}"));
        }
        println!(
            "  误报（javac 接受、分析器报错，或多出 unchecked 警告）: {}",
            report.false_positives.len()
        );
        for item in &report.false_positives {
            println!("      {item}");
            problems.push(format!("误报: {item}"));
        }
        println!(
            "  多报（变体里分析器报在了 javac 没报的行上）: {}",
            report.extra_reports.len()
        );
        for item in report.extra_reports.iter().take(20) {
            println!("      {item}");
        }
        println!(
            "  没有造成错误的变体: {} / {}",
            report.benign_mutants, report.mutants
        );

        println!(
            "\n  {:<18} {:>10} {:>8} {:>8} {:>8}",
            "javac 的错误类别", "javac 报错", "检出", "类别一致", "检出率"
        );
        for (category, tally) in &report.by_category {
            println!(
                "  {:<18} {:>10} {:>8} {:>8} {:>8}",
                category,
                tally.javac_errors,
                tally.detected,
                tally.exact,
                percent(tally.detected, tally.javac_errors)
            );
        }
        let total = report.supported_totals();
        println!(
            "  {:<18} {:>10} {:>8} {:>8} {:>8}",
            "合计（受支持的）",
            total.javac_errors,
            total.detected,
            total.exact,
            percent(total.detected, total.javac_errors)
        );

        println!(
            "\n  {:<18} {:>10} {:>8} {:>8}   说明",
            "变异算子", "javac 报错", "检出", "检出率"
        );
        let descriptions: BTreeMap<_, _> = javac::mutator_descriptions().into_iter().collect();
        for (mutator, tally) in &report.by_mutator {
            println!(
                "  {:<18} {:>10} {:>8} {:>8}   {}",
                mutator,
                tally.javac_errors,
                tally.detected,
                percent(tally.detected, tally.javac_errors),
                descriptions.get(mutator.as_str()).copied().unwrap_or("")
            );
        }
        println!(
            "\n  {:<18} {:>10} {:>8} {:>8}",
            "unchecked 警告", "javac 报的行", "检出", "检出率"
        );
        for (source, tally) in &report.unchecked {
            println!(
                "  {:<18} {:>10} {:>8} {:>8}",
                source,
                tally.javac_errors,
                tally.detected,
                percent(tally.detected, tally.javac_errors)
            );
        }
        if !report.missed_examples.is_empty() {
            println!("\n  没检出的例子:");
            for example in &report.missed_examples {
                println!("      {example}");
            }
        }
        javac_report = Some(report);
    }

    // ---- 基线 ----
    let baseline_path = options.dir.join("baseline.json");
    let current = Baseline {
        scenarios: score(&results),
        javac_detected: javac_report
            .as_ref()
            .map_or(0, |r| r.supported_totals().detected),
        javac_errors: javac_report
            .as_ref()
            .map_or(0, |r| r.supported_totals().javac_errors),
        javac_unchecked_detected: javac_report
            .as_ref()
            .map_or(0, |r| r.unchecked.values().map(|t| t.detected).sum()),
        javac_unchecked_warnings: javac_report
            .as_ref()
            .map_or(0, |r| r.unchecked.values().map(|t| t.javac_errors).sum()),
    };
    let full_run = options.mode == "all" && options.filter.is_none();
    if options.update_baseline {
        anyhow::ensure!(
            full_run,
            "--update-baseline 只能在不带过滤条件的完整运行（all）之后使用"
        );
        std::fs::write(
            &baseline_path,
            serde_json::to_string_pretty(&current)? + "\n",
        )?;
        println!("\n基线已写入 {}", baseline_path.display());
    } else if full_run {
        match std::fs::read_to_string(&baseline_path)
            .ok()
            .and_then(|s| serde_json::from_str::<Baseline>(&s).ok())
        {
            None => println!(
                "\n没有基线（{}）；用 --update-baseline 生成",
                baseline_path.display()
            ),
            Some(baseline) => {
                for (category, old) in &baseline.scenarios {
                    let new = current.scenarios.get(category).cloned().unwrap_or_default();
                    if new.passed < old.passed {
                        problems.push(format!(
                            "类别 {category} 通过的场景从 {} 降到 {}",
                            old.passed, new.passed
                        ));
                    }
                }
                // 语料变了的话两次的数字不可比，只在 javac 报错总数一致时比较
                if baseline.javac_errors == current.javac_errors
                    && current.javac_detected < baseline.javac_detected
                {
                    problems.push(format!(
                        "javac 差分的检出数从 {} 降到 {}",
                        baseline.javac_detected, current.javac_detected
                    ));
                }
                if baseline.javac_unchecked_warnings == current.javac_unchecked_warnings
                    && current.javac_unchecked_detected < baseline.javac_unchecked_detected
                {
                    problems.push(format!(
                        "unchecked 警告的检出数从 {} 降到 {}",
                        baseline.javac_unchecked_detected, current.javac_unchecked_detected
                    ));
                }
                if baseline.javac_errors != current.javac_errors {
                    println!("\n语料或变异算子变了（javac 报错数 {} → {}），检出数不与基线比较；确认后用 --update-baseline 更新", baseline.javac_errors, current.javac_errors);
                } else {
                    println!("\n与基线一致或更好（{}）", baseline_path.display());
                }
            }
        }
    }

    if let Some(path) = &options.json {
        let report = FullReport {
            jdk: &env.jdk,
            scenarios: results
                .iter()
                .map(|r| ScenarioJson {
                    name: &r.name,
                    category: &r.category,
                    checks: r.checks,
                    passed: r.passed(),
                    xfail: r.xfail.as_deref(),
                    failures: &r.failures,
                })
                .collect(),
            javac: javac_report.as_ref(),
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(&report)?)?;
        println!("完整结果: {}", path.display());
    }

    if problems.is_empty() {
        println!("\n评测通过");
        Ok(())
    } else {
        println!("\n评测未通过:");
        for problem in &problems {
            println!("  - {problem}");
        }
        std::process::exit(1);
    }
}
