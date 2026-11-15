//! 解析性能基准：`cargo bench -p syntax`。
//!
//! - `parse/full`：整文件解析一个约 3000 行的合成源文件；
//! - `parse/incremental`：在其中一个方法体里改一个字符后的块级增量重解析；
//! - `parse/corpus`：解析 `test_data/ok` 下的语料文件。

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use syntax::{ast::CompilationUnit, diff_range};

/// 生成一个有 `methods` 个方法的类，覆盖常见的语句和表达式形态。
fn synthetic_source(methods: usize) -> String {
    let mut out = String::from(
        "package bench;\n\nimport java.util.ArrayList;\nimport java.util.List;\nimport java.util.Map;\n\npublic class Bench {\n",
    );
    for i in 0..methods {
        out.push_str(&format!(
            r#"
    /** Method number {i}. */
    public <T extends Comparable<T>> List<T> method{i}(Map<String, List<T>> input, int limit) {{
        List<T> result = new ArrayList<>();
        for (Map.Entry<String, List<T>> entry : input.entrySet()) {{
            if (entry.getKey().length() > limit && !entry.getValue().isEmpty()) {{
                result.addAll(entry.getValue());
            }} else if (entry.getKey().startsWith("skip")) {{
                continue;
            }}
            int total = 0;
            for (int k = 0; k < limit; k++) {{
                total += k * {i} + (k << 2) - (total >> 1);
            }}
            String label = switch (total % 3) {{
                case 0 -> "zero";
                case 1 -> "one";
                default -> String.valueOf(total);
            }};
            Runnable task = () -> System.out.println(label + entry.getKey());
            task.run();
        }}
        return result;
    }}
"#
        ));
    }
    out.push_str("}\n");
    out
}

fn corpus() -> Vec<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("test_data")
        .join("ok");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("读取语料目录")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "java"))
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .collect()
}

fn bench_parse(c: &mut Criterion) {
    let source = synthetic_source(130);
    let mut group = c.benchmark_group("parse");

    group.throughput(Throughput::Bytes(source.len() as u64));
    group.bench_function("full", |b| {
        b.iter(|| CompilationUnit::parse(black_box(&source)));
    });

    // 在文件中部的一个方法体里把 `total += k` 改成 `total += kk`
    let old_parse = CompilationUnit::parse(&source);
    let anchor = source
        .match_indices("total += k")
        .nth(65)
        .map(|(idx, _)| idx)
        .expect("锚点");
    let mut edited = source.clone();
    edited.insert(anchor + "total += k".len(), 'k');
    let edit = diff_range(&source, &edited).expect("文本不同");
    assert!(
        old_parse.reparse(edit, &edited).is_some(),
        "基准里的编辑应当走增量路径"
    );
    group.bench_function("incremental", |b| {
        b.iter(|| old_parse.reparse(black_box(edit), black_box(&edited)));
    });

    let files = corpus();
    let bytes: usize = files.iter().map(String::len).sum();
    group.throughput(Throughput::Bytes(bytes as u64));
    group.bench_function("corpus", |b| {
        b.iter(|| {
            for file in &files {
                black_box(CompilationUnit::parse(file));
            }
        });
    });
    group.finish();
}

criterion_group!(benches, bench_parse);
criterion_main!(benches);
