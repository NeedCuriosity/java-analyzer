//! 按新签名更新用法的测试。

use syntax::TextRange;
use test_fixture::fixture;

/// 带光标的文件里，把 `edits` 的每一项（新文本 → 旧文本）换回去得到编辑之前的文本，
/// 再在光标处执行"按新签名更新用法"。返回修改后的 `路径 → 全文`。
fn change_signature(
    fixture_text: &str,
    edits: &[(&str, &str)],
) -> Option<Result<Vec<(String, String)>, String>> {
    let fx = fixture(fixture_text);
    let (source, file_id, offset) = fx.position();
    let after: &str = source.text(&fx.db);
    let mut before = after.to_owned();
    for (new, old) in edits {
        assert!(before.contains(new), "{new:?}");
        before = before.replacen(new, old, 1);
    }
    let tracker = crate::SignatureTracker::from_edit(&before, after);
    let range = crate::FileRange {
        file_id,
        range: TextRange::empty(offset),
    };
    let update = crate::change_signature::change_signature(&fx.db, range, &tracker)?;
    Some(update.change.map(|change| {
        let mut res = Vec::new();
        for (file_id, edits) in &change.edits {
            let file = fx
                .files
                .iter()
                .find(|f| f.file_id == *file_id)
                .expect("修改了 fixture 之外的文件");
            let mut text = file.source.text(&fx.db).to_string();
            for edit in edits.iter().rev() {
                text.replace_range(
                    usize::from(edit.range.start())..usize::from(edit.range.end()),
                    &edit.new_text,
                );
            }
            res.push((file.path.clone(), text));
        }
        res.sort();
        res
    }))
}

fn file_text<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    files
        .iter()
        .find(|(p, _)| p == path)
        .map(|(_, t)| t.as_str())
        .unwrap_or_else(|| panic!("{path} 没有被修改: {files:?}"))
}

#[test]
fn updates_calls_overrides_and_docs() {
    let files = change_signature(
        r#"
//- /a/Shape.java
package a;
public abstract class Shape {
    /**
     * Scales the shape.
     *
     * @param factor the factor
     * @param label the label
     */
    public abstract void sc$0ale(String label, double factor, boolean round);
}
//- /a/Circle.java
package a;
public class Circle extends Shape {
    /**
     * @param f the factor
     * @param label the label
     */
    @Override
    public void scale(double f, String label) {
        System.out.println(label + f);
    }
}
//- /b/Main.java
package b;
import a.*;
class Main {
    double compute() { return 1.0; }
    void run(Shape s) {
        s.scale(2.0, "x");
        new Circle().scale(compute(), "y");
        boolean round = true;
        s.scale(1.0, "z");
    }
}
"#,
        &[(
            "String label, double factor, boolean round",
            "double factor, String label",
        )],
    )
    .expect("应当提供")
    .expect("应当可以完成");
    let shape = file_text(&files, "/a/Shape.java");
    assert!(
        shape.contains(
            "     * @param label the label\n     * @param factor the factor\n     * @param round\n     */"
        ),
        "{shape}"
    );
    let circle = file_text(&files, "/a/Circle.java");
    assert!(
        circle.contains("public void scale(String label, double f, boolean round) {"),
        "{circle}"
    );
    assert!(
        circle.contains(
            "     * @param label the label\n     * @param f the factor\n     * @param round\n"
        ),
        "{circle}"
    );
    let main = file_text(&files, "/b/Main.java");
    assert!(main.contains("s.scale(\"x\", 2.0, false);"), "{main}");
    assert!(
        main.contains("new Circle().scale(\"y\", compute(), false);"),
        "{main}"
    );
    assert!(main.contains("s.scale(\"z\", 1.0, round);"), "{main}");
}

#[test]
fn refuses_to_drop_side_effects() {
    let result = change_signature(
        r#"
class Main {
    int next() { return 1; }
    void f$0(int a) {}
    void g() { f(1, next()); }
}
"#,
        &[("void f(int a)", "void f(int a, int b)")],
    )
    .expect("应当提供");
    let err = result.expect_err("删除有副作用的实参应当被拒绝");
    assert!(err.contains("next()"), "{err}");
}

#[test]
fn renames_method_parameters_and_method_refs() {
    let files = change_signature(
        r#"
interface Fn { int apply(int v); }
class Main {
    int doub$0led(int value) { return x * 2; }
    int run() {
        Fn f = this::twice;
        return twice(3) + f.apply(1);
    }
}
"#,
        &[("int doubled(int value)", "int twice(int x)")],
    )
    .unwrap()
    .unwrap();
    let main = &files[0].1;
    assert!(
        main.contains("int doubled(int value) { return value * 2; }"),
        "{main}"
    );
    assert!(main.contains("Fn f = this::doubled;"), "{main}");
    assert!(main.contains("return doubled(3) + f.apply(1);"), "{main}");
}

#[test]
fn refuses_method_refs_that_no_longer_fit() {
    let result = change_signature(
        r#"
interface Fn { int apply(int v); }
class Main {
    int tw$0ice(int x, int y) { return x * 2; }
    int run() {
        Fn f = this::twice;
        return f.apply(1);
    }
}
"#,
        &[("int twice(int x, int y)", "int twice(int x)")],
    )
    .unwrap();
    assert!(result.expect_err("方法引用对不上").contains("lambda"));
}

#[test]
fn constructor_calls() {
    let files = change_signature(
        r#"
//- /Point.java
class Point {
    Po$0int(int x, int y, int z) {}
    Point() { this(0, 0); }
}
//- /Point3.java
class Point3 extends Point {
    Point3(int a) { super(a, a); }
}
//- /Main.java
class Main {
    Point p = new Point(1, 2);
}
"#,
        &[("Point(int x, int y, int z)", "Point(int x, int y)")],
    )
    .unwrap()
    .unwrap();
    let point = file_text(&files, "/Point.java");
    assert!(point.contains("Point() { this(0, 0, 0); }"), "{point}");
    let sub = file_text(&files, "/Point3.java");
    assert!(sub.contains("super(a, a, 0);"), "{sub}");
    let main = file_text(&files, "/Main.java");
    assert!(main.contains("new Point(1, 2, 0);"), "{main}");
}

#[test]
fn enum_constructor_arguments() {
    let files = change_signature(
        r#"
enum Color {
    RED(1), GREEN(2) { },
    BLUE;
    Col$0or(int rgb, boolean dark) {}
    Color() { this(0); }
}
"#,
        &[("Color(int rgb, boolean dark)", "Color(int rgb)")],
    )
    .unwrap()
    .unwrap();
    let text = &files[0].1;
    assert!(
        text.contains("RED(1, false), GREEN(2, false) { },"),
        "{text}"
    );
    assert!(text.contains("BLUE;"), "{text}");
    assert!(text.contains("Color() { this(0, false); }"), "{text}");
}

#[test]
fn anonymous_and_diamond_implementations() {
    let files = change_signature(
        r#"
//- /Fn.java
interface Fn<T> {
    void app$0ly(T t, int times);
}
//- /Main.java
class Main {
    void run() {
        Fn<String> a = new Fn<>() {
            @Override
            public void apply(String t) {}
        };
        Fn<String> b = new Fn<String>() {
            @Override
            public void apply(String s) {}
        };
        a.apply("x");
        b.apply("y");
    }
}
"#,
        &[("void apply(T t, int times)", "void apply(T t)")],
    )
    .unwrap()
    .unwrap();
    let main = file_text(&files, "/Main.java");
    assert!(
        main.contains("public void apply(String t, int times) {}"),
        "{main}"
    );
    assert!(
        main.contains("public void apply(String s, int times) {}"),
        "{main}"
    );
    assert!(main.contains("a.apply(\"x\", 0);"), "{main}");
    assert!(main.contains("b.apply(\"y\", 0);"), "{main}");
}

#[test]
fn varargs_stay_last() {
    let files = change_signature(
        r#"
class Log {
    void lo$0g(int level, String fmt, Object... args) {}
    void run() {
        log("a %s %s", 1, 2);
        log("none");
    }
}
"#,
        &[(
            "void log(int level, String fmt, Object... args)",
            "void log(String fmt, Object... args)",
        )],
    )
    .unwrap()
    .unwrap();
    let text = &files[0].1;
    assert!(text.contains("log(0, \"a %s %s\", 1, 2);"), "{text}");
    assert!(text.contains("log(0, \"none\");"), "{text}");
}

#[test]
fn varargs_turned_into_an_array() {
    let files = change_signature(
        r#"
class Sum {
    int su$0m(int[] xs) { return 0; }
    void run(int[] given) {
        sum(1, 2, 3);
        sum(given);
    }
}
"#,
        &[("int sum(int[] xs)", "int sum(int... xs)")],
    )
    .unwrap()
    .unwrap();
    let text = &files[0].1;
    assert!(text.contains("sum(new int[]{1, 2, 3});"), "{text}");
    assert!(text.contains("sum(given);"), "{text}");
}

#[test]
fn overload_resolution_uses_the_old_signature() {
    let files = change_signature(
        r#"
class Box {
    void p$0ut(int a, int b) {}
    void put(String s) {}
    void run() {
        put(1);
        put("s");
    }
}
"#,
        &[("void put(int a, int b)", "void put(int a)")],
    )
    .unwrap()
    .unwrap();
    let text = &files[0].1;
    assert!(text.contains("put(1, 0);"), "{text}");
    assert!(text.contains("put(\"s\");"), "{text}");
}

#[test]
fn nested_calls_are_rewritten_together() {
    let files = change_signature(
        r#"
class Calc {
    int ad$0d(int b, int a) { return a + b; }
    int run() { return add(add(1, 2), 3); }
}
"#,
        &[("int add(int b, int a)", "int add(int a, int b)")],
    )
    .unwrap()
    .unwrap();
    let text = &files[0].1;
    assert!(text.contains("return add(3, add(2, 1));"), "{text}");
}

#[test]
fn not_offered_without_a_signature_change() {
    assert!(change_signature(
        "class Main {\n    void f$0(int a) { int x = 2; }\n}\n",
        &[("int x = 2;", "int x = 1;")],
    )
    .is_none());
    assert!(change_signature("class Main {\n    void f$0(int a) {}\n}\n", &[]).is_none());
}

#[test]
fn types_and_return_types_in_other_packages() {
    let files = change_signature(
        r#"
//- /a/Item.java
package a;
public class Item {}
//- /a/Out.java
package a;
public class Out {}
//- /a/Base.java
package a;
public interface Base {
    Out ma$0ke(Item item);
}
//- /b/Impl.java
package b;

class Impl implements a.Base {
    public Object make(String item) { return null; }
}
"#,
        &[("Out make(Item item)", "Object make(String item)")],
    )
    .unwrap()
    .unwrap();
    let text = file_text(&files, "/b/Impl.java");
    assert!(text.contains("public Out make(Item item)"), "{text}");
    assert!(text.contains("import a.Item;"), "{text}");
    assert!(text.contains("import a.Out;"), "{text}");
}

#[test]
fn renamed_static_method_and_its_static_import() {
    let files = change_signature(
        r#"
//- /a/Util.java
package a;
public class Util {
    public static int tw$0ice(int x) { return x; }
}
//- /b/Main.java
package b;
import static a.Util.doubled;
class Main {
    int f() { return doubled(1) + a.Util.doubled(2); }
}
"#,
        &[("int twice(int x)", "int doubled(int x)")],
    )
    .unwrap()
    .unwrap();
    let text = file_text(&files, "/b/Main.java");
    assert!(text.contains("import static a.Util.twice;"), "{text}");
    assert!(
        text.contains("return twice(1) + a.Util.twice(2);"),
        "{text}"
    );
}

#[test]
fn super_calls_in_overrides() {
    let files = change_signature(
        r#"
//- /Base.java
class Base {
    void hel$0lo(String who, int times) {}
}
//- /Sub.java
class Sub extends Base {
    @Override
    void hello(String who) {
        super.hello(who);
    }
}
"#,
        &[(
            "void hello(String who, int times)",
            "void hello(String who)",
        )],
    )
    .unwrap()
    .unwrap();
    let text = file_text(&files, "/Sub.java");
    assert!(
        text.contains("void hello(String who, int times) {"),
        "{text}"
    );
    assert!(text.contains("super.hello(who, times);"), "{text}");
}
