//! `analysis-stats` 的补全命中率：在真实代码里抽取标识符出现的位置，把标识符删到只剩前缀，
//! 像在编辑器里打字那样用改过的文本请求补全，看原来的名字有没有出现、排在第几。
//!
//! 抽样是确定的：按路径顺序等距取文件，每个文件里每种位置等距取几个。
//! `JA_STATS_COMPLETION_FILES=N` 控制取多少个文件（默认 80，0 表示不测），
//! `JA_STATS_COMPLETION_MISSES=N` 时列出前 N 个没出现的和排在前 5 之外的样例。

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use ide::{AnalysisHost, Change, FileChange, FileId, FileKind};
use rustc_hash::FxHashSet;
use syntax::{
    ast::{self, AstNode},
    SyntaxKind,
};
use vfs::Vfs;

/// 标识符所处的位置种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SiteKind {
    /// `x.name`（不是调用）
    Member,
    /// `x.name(...)`
    QualifiedCall,
    /// `name(...)`
    BareCall,
    /// 表达式里单独的名字：局部变量、字段、类名
    Name,
    /// 类型位置的类名
    Type,
    /// `new X(...)` 的 `X`
    New,
    /// `@X`
    Annotation,
}

impl SiteKind {
    const ALL: [SiteKind; 7] = [
        SiteKind::Member,
        SiteKind::QualifiedCall,
        SiteKind::BareCall,
        SiteKind::Name,
        SiteKind::Type,
        SiteKind::New,
        SiteKind::Annotation,
    ];

    fn label(self) -> &'static str {
        match self {
            SiteKind::Member => base_db::trs!("成员访问 x.f", "member x.f"),
            SiteKind::QualifiedCall => base_db::trs!("方法调用 x.m()", "call x.m()"),
            SiteKind::BareCall => base_db::trs!("无接收者调用 m()", "bare call m()"),
            SiteKind::Name => base_db::trs!("名字引用", "name"),
            SiteKind::Type => base_db::trs!("类型名", "type"),
            SiteKind::New => base_db::trs!("new 类型", "new type"),
            SiteKind::Annotation => base_db::trs!("注解名", "annotation"),
        }
    }
}

struct Site {
    kind: SiteKind,
    /// 标识符在原文里的字节范围
    start: usize,
    end: usize,
}

/// 文件里所有可以抽样的标识符位置，按出现顺序。
fn collect_sites(text: &str) -> Vec<Site> {
    let root = ast::CompilationUnit::parse(text).syntax_node();
    let mut sites = Vec::new();
    for node in root.descendants() {
        let Some(name_ref) = ast::NameRef::cast(node) else {
            continue;
        };
        let Some(parent) = name_ref.syntax().parent() else {
            continue;
        };
        let grandparent = parent.parent().map(|p| p.kind());
        let kind = match parent.kind() {
            SyntaxKind::FIELD_ACCESS => SiteKind::Member,
            SyntaxKind::METHOD_CALL => match ast::MethodCall::cast(parent.clone()) {
                Some(call) if call.receiver().is_some() => SiteKind::QualifiedCall,
                Some(_) => SiteKind::BareCall,
                None => continue,
            },
            SyntaxKind::NAME_EXPR => SiteKind::Name,
            // 限定类型 `a.b.C` 里面的 `a`、`b` 是包名或外层类，只取最后一段
            SyntaxKind::CLASS_TYPE if grandparent == Some(SyntaxKind::CLASS_TYPE) => continue,
            SyntaxKind::CLASS_TYPE if grandparent == Some(SyntaxKind::NEW_EXPR) => SiteKind::New,
            SyntaxKind::CLASS_TYPE => SiteKind::Type,
            SyntaxKind::QUALIFIED_NAME if grandparent == Some(SyntaxKind::ANNOTATION) => {
                SiteKind::Annotation
            }
            _ => continue,
        };
        let Some(token) = name_ref.ident_token() else {
            continue;
        };
        if token.kind() != SyntaxKind::IDENT {
            continue;
        }
        let range = token.text_range();
        sites.push(Site {
            kind,
            start: range.start().into(),
            end: range.end().into(),
        });
    }
    sites
}

#[derive(Default, Clone)]
struct Tally {
    samples: usize,
    found: usize,
    top1: usize,
    top5: usize,
    rank_sum: usize,
    times: Vec<Duration>,
}

impl Tally {
    fn add(&mut self, rank: Option<usize>, time: Duration) {
        self.samples += 1;
        self.times.push(time);
        if let Some(rank) = rank {
            self.found += 1;
            self.rank_sum += rank;
            if rank == 1 {
                self.top1 += 1;
            }
            if rank <= 5 {
                self.top5 += 1;
            }
        }
    }

    fn merge(&mut self, other: &Tally) {
        self.samples += other.samples;
        self.found += other.found;
        self.top1 += other.top1;
        self.top5 += other.top5;
        self.rank_sum += other.rank_sum;
        self.times.extend_from_slice(&other.times);
    }

    fn p50(&self) -> Duration {
        let mut times = self.times.clone();
        times.sort();
        times.get(times.len() / 2).copied().unwrap_or_default()
    }
}

/// 前缀的两种取法：什么都不留，留 1~2 个字符（名字只有 1~2 个字符时留 1 个）。
const MODES: [usize; 2] = [0, 2];

fn prefix_chars(mode: usize, name: &str) -> usize {
    if mode == 0 {
        return 0;
    }
    let len = name.chars().count();
    mode.min(len.saturating_sub(1)).max(1)
}

fn mode_label(mode: usize) -> String {
    if mode == 0 {
        base_db::trs!("前缀为空", "empty prefix").to_owned()
    } else {
        base_db::tr!("前缀 1~{mode} 个字符", "prefix of 1-{mode} chars")
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

pub(super) fn completion_hit_rate(
    host: &mut AnalysisHost,
    vfs: &Vfs,
    files: &[(FileId, FileKind)],
) -> anyhow::Result<()> {
    const PER_FILE: usize = 2;
    let max_files = env_usize("JA_STATS_COMPLETION_FILES", 80);
    let max_misses = env_usize("JA_STATS_COMPLETION_MISSES", 0);
    if max_files == 0 || files.is_empty() {
        return Ok(());
    }
    let config = ide::CompletionConfig::default();
    let step = (files.len() / max_files).max(1);
    let mut tallies: Vec<Vec<Tally>> =
        vec![vec![Tally::default(); SiteKind::ALL.len()]; MODES.len()];
    let mut missing: Vec<String> = Vec::new();
    let mut low: Vec<String> = Vec::new();
    let mut latencies: Vec<Duration> = Vec::new();
    let start = Instant::now();

    for &(file_id, kind) in files.iter().step_by(step).take(max_files) {
        let Some(original) = host.analysis().file_text(file_id)? else {
            continue;
        };
        let path = vfs.file_path(file_id).display().to_string();
        let name = base_db::file_stem(&path);
        let dir = base_db::file_dir(&path);
        let sites = collect_sites(&original);
        let mut chosen: Vec<&Site> = Vec::new();
        for site_kind in SiteKind::ALL {
            let of_kind: Vec<&Site> = sites.iter().filter(|s| s.kind == site_kind).collect();
            let stride = (of_kind.len() / PER_FILE).max(1);
            chosen.extend(
                of_kind
                    .into_iter()
                    .skip(stride / 2)
                    .step_by(stride)
                    .take(PER_FILE),
            );
        }
        if chosen.is_empty() {
            continue;
        }
        let set_text = |host: &mut AnalysisHost, text: Arc<str>| {
            host.apply_change(Change {
                files: vec![(
                    file_id,
                    Some(FileChange {
                        text,
                        kind,
                        name: name.clone(),
                        dir: dir.clone(),
                    }),
                )],
                ..Change::default()
            });
        };
        for site in chosen {
            let expected = &original[site.start..site.end];
            let kind_idx = SiteKind::ALL
                .iter()
                .position(|k| *k == site.kind)
                .unwrap_or(0);
            for (mode_idx, mode) in MODES.iter().enumerate() {
                let keep: usize = expected
                    .chars()
                    .take(prefix_chars(*mode, expected))
                    .map(char::len_utf8)
                    .sum();
                let cursor = site.start + keep;
                let mut text = String::with_capacity(original.len());
                text.push_str(&original[..cursor]);
                text.push_str(&original[site.end..]);
                set_text(host, Arc::from(text.as_str()));

                let analysis = host.analysis();
                let position = ide::FilePosition {
                    file_id,
                    offset: syntax::TextSize::from(cursor as u32),
                };
                let request = Instant::now();
                let list = analysis.completions(position, &config)?;
                let elapsed = request.elapsed();
                latencies.push(elapsed);

                // 重载的方法、同名的类各算一个名字：排名按去重后的名字算
                let mut seen: FxHashSet<&str> = FxHashSet::default();
                let mut top: Vec<&str> = Vec::new();
                let mut rank = None;
                for item in &list.items {
                    if !seen.insert(item.lookup.as_str()) {
                        continue;
                    }
                    if top.len() < 5 {
                        top.push(&item.label);
                    }
                    if item.lookup == expected {
                        rank = Some(seen.len());
                        break;
                    }
                }
                if rank.is_none() {
                    for item in &list.items {
                        if top.len() >= 5 {
                            break;
                        }
                        if !top.contains(&item.label.as_str()) {
                            top.push(&item.label);
                        }
                    }
                }
                tallies[mode_idx][kind_idx].add(rank, elapsed);

                let bucket = match rank {
                    None => Some(&mut missing),
                    Some(r) if r > 5 => Some(&mut low),
                    Some(_) => None,
                };
                if let Some(bucket) = bucket.filter(|b| b.len() < max_misses) {
                    let line_start = text[..cursor].rfind('\n').map_or(0, |i| i + 1);
                    let line_end = text[cursor..].find('\n').map_or(text.len(), |i| cursor + i);
                    let line_no = text[..cursor].matches('\n').count() + 1;
                    let context: String =
                        format!("{}|{}", &text[line_start..cursor], &text[cursor..line_end])
                            .trim()
                            .chars()
                            .take(120)
                            .collect();
                    let rank_text = rank.map(|r| format!(" #{r}")).unwrap_or_default();
                    bucket.push(format!(
                        "  [{}] [{}] {path}:{line_no}: `{expected}`{rank_text}  {context}\n      -> {}",
                        mode_label(*mode),
                        site.kind.label(),
                        top.join(", ")
                    ));
                }
            }
        }
        set_text(host, original.clone());
    }

    println!(
        "{}",
        base_db::tr!(
            "补全命中率（抽样，删掉标识符后在原位置补全，排名按去重后的名字算；{:.2?}）:",
            "Completion hit rate (sampled; identifier deleted and completed in place, ranks over distinct names; {:.2?}):",
            start.elapsed()
        )
    );
    for (mode_idx, mode) in MODES.iter().enumerate() {
        println!("  {}", mode_label(*mode));
        println!(
            "{}",
            base_db::trs!(
                "    位置                   样本   出现    第1    前5  平均排名  耗时p50",
                "    site                samples  found   top1   top5  avg rank  p50 time"
            )
        );
        let mut total = Tally::default();
        let row = |label: &str, t: &Tally| {
            let pct = |n: usize| n as f64 * 100.0 / t.samples.max(1) as f64;
            let avg = t.rank_sum as f64 / t.found.max(1) as f64;
            println!(
                "    {label} {:>6} {:>5.1}% {:>5.1}% {:>5.1}% {:>8.2} {:>9.2?}",
                t.samples,
                pct(t.found),
                pct(t.top1),
                pct(t.top5),
                avg,
                t.p50()
            );
        };
        for (kind_idx, site_kind) in SiteKind::ALL.iter().enumerate() {
            let tally = &tallies[mode_idx][kind_idx];
            total.merge(tally);
            if tally.samples > 0 {
                row(&pad(site_kind.label(), 20), tally);
            }
        }
        row(&pad(base_db::trs!("合计", "total"), 20), &total);
    }
    if !latencies.is_empty() {
        latencies.sort();
        let at =
            |p: f64| latencies[((latencies.len() as f64 * p) as usize).min(latencies.len() - 1)];
        println!(
            "{}",
            base_db::tr!(
                "  补全耗时 p50 {:.2?}  p95 {:.2?}  最大 {:.2?}（{} 次，含改动后的重新分析）",
                "  Completion time p50 {:.2?}  p95 {:.2?}  max {:.2?} ({} requests, including re-analysis after the edit)",
                at(0.50),
                at(0.95),
                at(1.0),
                latencies.len()
            )
        );
    }
    if max_misses > 0 {
        println!(
            "{}",
            base_db::trs!("  没出现在结果里:", "  Missing from the results:")
        );
        for line in &missing {
            println!("{line}");
        }
        println!("{}", base_db::trs!("  排在前 5 之外:", "  Ranked below 5:"));
        for line in &low {
            println!("{line}");
        }
    }
    Ok(())
}

/// 按显示宽度补齐（中文字符占两格）。
fn pad(text: &str, width: usize) -> String {
    let shown: usize = text.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum();
    format!("{text}{}", " ".repeat(width.saturating_sub(shown)))
}
