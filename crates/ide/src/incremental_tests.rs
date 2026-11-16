//! 增量计算的失效范围：编辑之后哪些查询会重新执行。
//!
//! 打字时最常见的是在方法体里编辑。这里锁住的约定：方法体内的编辑只让这个体重新推断，
//! 同一文件的其它体（哪怕位置挪动了）、别的文件的推断和诊断都直接复用。

use std::sync::Arc;

use base_db::{salsa::Setter, SourceFile};
use ide_db::{record_executions, QueryExecutions, RootDatabase};
use ide_diagnostics::{DiagnosticCode, DiagnosticsConfig};
use test_fixture::{fixture, Fixture};

use crate::highlight;

const MAIN: &str = r#"
//- /p/Main.java
package p;

public class Main {
    public static final int LIMIT = 10;

    int first(int a) {
        int x = a + 1;
        return x;
    }

    int second() {
        String s = "abc";
        return s.length() + missing;
    }

    int third() {
        return first(LIMIT);
    }
}
//- /p/Other.java
package p;

class Other {
    int use(Main m) {
        if (Main.LIMIT > 5) {
            return m.third();
        }
        return 0;
    }
}
"#;

fn source(fx: &Fixture, path: &str) -> SourceFile {
    fx.file_by_path(path).source
}

/// 把 `file` 里第一处 `from` 换成 `to`。
fn replace(db: &mut RootDatabase, file: SourceFile, from: &str, to: &str) {
    let text = file.text(db).to_string();
    assert!(text.contains(from), "文件里没有 {from:?}");
    let new: Arc<str> = Arc::from(text.replacen(from, to, 1));
    file.set_text(db).to(new);
}

fn diagnostics_of(db: &RootDatabase, file: SourceFile) -> (Vec<String>, QueryExecutions) {
    let (diags, executions) = record_executions(db, || {
        ide_diagnostics::diagnostics(db, file, &DiagnosticsConfig::default())
    });
    let text = file.text(db);
    let rendered = diags
        .iter()
        .filter(|d| d.code != DiagnosticCode::UnusedImport)
        .map(|d| format!("{:?} {}", d.code, &text[d.range]))
        .collect();
    (rendered, executions)
}

fn warm(fx: &Fixture) {
    for file in [source(fx, "/p/Main.java"), source(fx, "/p/Other.java")] {
        ide_diagnostics::diagnostics(&fx.db, file, &DiagnosticsConfig::default());
        highlight::highlight(&fx.db, file, None);
    }
}

#[test]
fn body_edit_reinfers_only_that_body() {
    let mut fx = fixture(MAIN);
    warm(&fx);
    let main = source(&fx, "/p/Main.java");
    let (before, _) = diagnostics_of(&fx.db, main);

    // 在第一个方法体里插入一条语句：后面两个方法整体挪了位置
    replace(
        &mut fx.db,
        main,
        "int x = a + 1;",
        "int y = 0; int x = a + 1;",
    );

    let (after, executions) = diagnostics_of(&fx.db, main);
    assert_eq!(executions.count("infer_tracked"), 1, "{executions:?}");
    // 签名级的摘要没变：类的签名、类索引都不重新计算
    assert_eq!(executions.count("class_data_tracked"), 0, "{executions:?}");
    assert_eq!(executions.count("source_class_index"), 0, "{executions:?}");
    // 挪了位置的方法里的诊断换算到新位置上，内容不变
    assert!(
        after.contains(&"UnresolvedName missing".to_owned()),
        "{after:?}"
    );
    assert_eq!(
        before.iter().filter(|d| d.contains("missing")).count(),
        after.iter().filter(|d| d.contains("missing")).count()
    );

    // 依赖它的文件（用到了它的常量和方法）的诊断直接复用
    let other = source(&fx, "/p/Other.java");
    let (_, executions) = diagnostics_of(&fx.db, other);
    assert_eq!(executions.count("default_diagnostics"), 0, "{executions:?}");
    assert_eq!(executions.count("infer_tracked"), 0, "{executions:?}");
}

#[test]
fn shifted_bodies_keep_their_results_at_new_positions() {
    let mut fx = fixture(MAIN);
    warm(&fx);
    let main = source(&fx, "/p/Main.java");
    let highlights = |db: &RootDatabase| {
        let text = main.text(db).to_string();
        highlight::highlight(db, main, None)
            .into_iter()
            .map(|h| format!("{} {:?}", &text[h.range], h.tag))
            .collect::<Vec<_>>()
    };
    let before = highlights(&fx.db);

    replace(&mut fx.db, main, "return x;", "return  x;");
    let (after, executions) = record_executions(&fx.db, || highlights(&fx.db));
    assert_eq!(executions.count("infer_tracked"), 1, "{executions:?}");
    assert_eq!(before, after);

    // 再改回去：只有被编辑的那个体重新推断
    replace(&mut fx.db, main, "return  x;", "return x;");
    let (_, executions) = record_executions(&fx.db, || highlights(&fx.db));
    assert!(executions.count("infer_tracked") <= 1, "{executions:?}");
}

#[test]
fn signature_edit_reinfers_dependents() {
    let mut fx = fixture(MAIN);
    warm(&fx);
    let main = source(&fx, "/p/Main.java");
    // 改了方法签名：用到它的别的文件要重新检查，并报出实参不匹配
    replace(&mut fx.db, main, "int third()", "int third(int k)");
    let other = source(&fx, "/p/Other.java");
    let (diags, executions) = diagnostics_of(&fx.db, other);
    assert!(executions.count("infer_tracked") >= 1, "{executions:?}");
    assert!(
        diags.iter().any(|d| d.starts_with("ArgumentMismatch")),
        "{diags:?}"
    );
}
