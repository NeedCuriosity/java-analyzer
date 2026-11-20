//! 用 JDK 的 javac 现场编译小段 Java 代码，再反编译回来检查。没有 JDK 时跳过。

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

use crate::{decompile_class_text, ClassEnv, DecompileStats};

struct MapEnv {
    classes: HashMap<String, Vec<u8>>,
}

impl ClassEnv for MapEnv {
    fn class_bytes(&self, binary_name: &str) -> Option<Vec<u8>> {
        self.classes.get(binary_name).cloned()
    }

    fn class_exists(&self, binary_name: &str) -> bool {
        self.classes.contains_key(binary_name)
            || matches!(
                binary_name,
                "java/lang/String" | "java/lang/Object" | "java/lang/Integer" | "java/lang/System"
            )
    }
}

fn javac() -> Option<PathBuf> {
    // Windows 上是 javac.exe
    let name = format!("javac{}", std::env::consts::EXE_SUFFIX);
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        let p = Path::new(&home).join("bin").join(&name);
        if p.is_file() {
            return Some(p);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(&name))
        .find(|p| p.is_file())
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn collect_classes(dir: &Path, root: &Path, out: &mut HashMap<String, Vec<u8>>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_classes(&p, root, out);
        } else if p.extension().is_some_and(|x| x == "class") {
            let rel = p
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let name = rel.trim_end_matches(".class").to_owned();
            out.insert(name, std::fs::read(&p).unwrap());
        }
    }
}

/// 编译 `File.java`，返回全部类文件。
fn compile(file: &str, src: &str, debug_info: bool) -> Option<HashMap<String, Vec<u8>>> {
    let javac = javac()?;
    let dir = std::env::temp_dir().join(format!(
        "ja-decompiler-test-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("out")).ok()?;
    std::fs::write(dir.join(file), src).ok()?;
    let mut cmd = Command::new(javac);
    cmd.current_dir(&dir)
        .args(["-proc:none", "-nowarn", "-d", "out"]);
    if debug_info {
        cmd.arg("-g");
    }
    let output = cmd.arg(file).output().ok()?;
    assert!(
        output.status.success(),
        "javac 失败:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut classes = HashMap::new();
    collect_classes(&dir.join("out"), &dir.join("out"), &mut classes);
    let _ = std::fs::remove_dir_all(&dir);
    Some(classes)
}

/// 反编译 `class`，检查没有语法错误、每个方法都反编译成功，返回源码。
fn decompile(src: &str, class: &str, debug_info: bool) -> Option<String> {
    let file = format!("{}.java", class.rsplit('/').next().unwrap());
    let classes = compile(&file, src, debug_info)?;
    let env = MapEnv { classes };
    let bytes = env.classes.get(class).expect("没有编译出这个类").clone();
    let mut stats = DecompileStats::default();
    let text = decompile_class_text(&bytes, &env, &mut stats).unwrap();
    if std::env::var_os("JA_SHOW").is_some() {
        eprintln!("{text}");
    }
    let parse = syntax::ast::CompilationUnit::parse(&text);
    assert!(
        parse.errors().is_empty(),
        "语法错误 {:?}:\n{text}",
        parse.errors()
    );
    assert!(
        stats.failures.is_empty(),
        "有方法没能反编译 {:?}:\n{text}",
        stats.failures
    );
    Some(text)
}

fn check(text: &str, needles: &[&str]) {
    for n in needles {
        assert!(text.contains(n), "缺少 `{n}`:\n{text}");
    }
}

const LOOPS: &str = r#"
import java.util.List;

public class Loops {
    int sum(int[] a) {
        int s = 0;
        for (int i = 0; i < a.length; i++) {
            s += a[i];
        }
        return s;
    }

    int collatz(int n) {
        int c = 0;
        while (n > 1) {
            n = n % 2 == 0 ? n / 2 : 3 * n + 1;
            c++;
        }
        return c;
    }

    int countDown(int n) {
        int c = 0;
        do {
            c++;
            n--;
        } while (n > 0);
        return c;
    }

    int lengths(List<String> xs) {
        int t = 0;
        for (String x : xs) {
            t += x.length();
        }
        return t;
    }

    int arraySum(int[] xs) {
        int t = 0;
        for (int x : xs) {
            t += x;
        }
        return t;
    }

    int find(int[][] m) {
        int found = -1;
        outer:
        for (int i = 0; i < m.length; i++) {
            for (int j = 0; j < m[i].length; j++) {
                if (m[i][j] < 0) {
                    found = i;
                    break outer;
                }
                if (m[i][j] == 0) {
                    continue outer;
                }
            }
            System.out.println(i);
        }
        return found;
    }

    void infinite(java.util.Iterator<String> it) {
        while (true) {
            if (!it.hasNext()) {
                return;
            }
            System.out.println(it.next());
        }
    }

    int search(int[] a, int k) {
        int e;
        int i = 0;
        for (;;) {
            if (i >= a.length) {
                e = -1;
                System.out.println("miss");
                break;
            }
            if (a[i] == k) {
                e = i;
                break;
            }
            i++;
        }
        if (e >= 0) {
            return e * 2;
        }
        return 0;
    }
}
"#;

#[test]
fn loops() {
    for debug in [true, false] {
        let Some(text) = decompile(LOOPS, "Loops", debug) else {
            return;
        };
        check(
            &text,
            &[
                "while (",
                "do {",
                "} while (",
                "break label",
                "continue label",
                " += ",
                "System.out.println(\"miss\");",
            ],
        );
        if debug {
            check(
                &text,
                &[
                    "for (int i = 0; i < a.length; i++)",
                    "for (String x : xs)",
                    "for (int x : xs)",
                    "n % 2 == 0 ? n / 2 : 3 * n + 1",
                ],
            );
        }
    }
}

const CONDS: &str = r#"
public class Conds {
    String classify(int x) {
        if (x < 0) {
            return "neg";
        } else if (x == 0) {
            return "zero";
        } else {
            return "pos";
        }
    }

    boolean both(boolean a, boolean b, int c) {
        return a && (b || c > 3);
    }

    int nested(int a, int b) {
        int r;
        if (a > 0) {
            if (b > 0) {
                r = 1;
            } else {
                r = 2;
            }
        } else {
            r = 3;
        }
        return r;
    }

    int ternary(int a) {
        return a > 10 ? a * 2 : a - 1;
    }

    String sw(int k) {
        switch (k) {
            case 1:
                return "one";
            case 2:
            case 3:
                return "few";
            default:
                return "many";
        }
    }

    int sw2(int k) {
        int r = 0;
        switch (k) {
            case 10:
                r = 1;
                break;
            case 20:
                r = 2;
            case 30:
                r += 3;
                break;
            default:
                r = -1;
        }
        return r;
    }

    String concat(String a, int b, char c) {
        return a + b + c + "!";
    }

    char first(String s) {
        return s.isEmpty() ? 'x' : s.charAt(0);
    }

    boolean flags(int x) {
        boolean even = x % 2 == 0;
        return even || x > 100;
    }

    long widen(int a, long b) {
        return a * b + 1L;
    }

    int[] arrays() {
        int[] xs = {1, 2, 3};
        String[][] grid = new String[2][3];
        return xs;
    }

    String old(String a, Object b) {
        StringBuilder sb = new StringBuilder();
        sb.append(a).append(':').append(b);
        return sb.toString();
    }
}
"#;

#[test]
fn conditionals() {
    for debug in [true, false] {
        let Some(text) = decompile(CONDS, "Conds", debug) else {
            return;
        };
        check(
            &text,
            &[
                "switch (",
                "case 2:",
                "case 3:",
                "default:",
                "? ",
                "+ \"!\"",
                "'x'",
                "new int[]{1, 2, 3}",
                "new String[2][3]",
            ],
        );
        // 没有调试信息时形参叫 `arg0` 之类，只在有调试信息时检查名字
        if debug {
            check(&text, &["if (x < 0)", "return a && (b || c > 3);"]);
        } else {
            check(
                &text,
                &["if (arg0 < 0)", "return arg0 && (arg1 || arg2 > 3);"],
            );
        }
    }
}

const EXC: &str = r#"
import java.io.IOException;
import java.io.InputStream;

public class Exc {
    int parse(String s) {
        try {
            return Integer.parseInt(s);
        } catch (NumberFormatException e) {
            return -1;
        }
    }

    void close(InputStream in) throws IOException {
        try {
            in.read();
        } finally {
            in.close();
        }
    }

    String multi(Object o) {
        try {
            return o.toString();
        } catch (IllegalStateException | IllegalArgumentException e) {
            return "bad";
        } finally {
            System.out.println("done");
        }
    }

    void sync(Object lock) {
        synchronized (lock) {
            System.out.println("in");
        }
    }

    private final Object mutex = new Object();
    private int count;

    int syncField() {
        synchronized (mutex) {
            return count;
        }
    }

    void syncTryFinally(Object lock, int[] a) {
        synchronized (lock) {
            try {
                a[0] = 1;
                if (a.length > 3) {
                    return;
                }
                a[1] = 2;
            } finally {
                a[2] = 3;
            }
        }
    }

    int syncLoop(Object lock, int[] a, int k) {
        int count = 0;
        synchronized (lock) {
            if (a.length > 0) {
                for (int i = 0;; i++) {
                    if (a[i] == k) {
                        count = i;
                        break;
                    }
                    if (i > 10) {
                        throw new IllegalStateException("x");
                    }
                }
            }
        }
        return count;
    }

    void syncThis(int n) {
        for (int i = 0; i < n; i++) {
            synchronized (this) {
                if (count > 10) {
                    break;
                }
                count += i;
            }
        }
    }

    void check(int x) {
        if (x < 0) {
            throw new IllegalArgumentException("negative: " + x);
        }
        assert x != 5 : "five";
    }

    int loopTry(int[] xs) {
        int bad = 0;
        for (int x : xs) {
            try {
                if (x == 0) {
                    continue;
                }
                bad += 10 / x;
            } catch (ArithmeticException e) {
                bad++;
            }
        }
        return bad;
    }
}
"#;

#[test]
fn exceptions() {
    for debug in [true, false] {
        let Some(text) = decompile(EXC, "Exc", debug) else {
            return;
        };
        check(
            &text,
            &[
                "catch (NumberFormatException ",
                "finally {",
                "IllegalStateException | IllegalArgumentException",
                "synchronized (",
                "synchronized (mutex) {",
                "synchronized (this) {",
                "throw new IllegalArgumentException(\"negative: \" + ",
                "assert ",
            ],
        );
    }
}

const LAMBDAS: &str = r#"
import java.util.ArrayList;
import java.util.Comparator;
import java.util.List;
import java.util.function.Function;
import java.util.function.Supplier;

public class Lambdas {
    private int base = 3;

    List<String> upper(List<String> xs) {
        List<String> out = new ArrayList<>();
        xs.forEach(x -> out.add(x.toUpperCase()));
        return out;
    }

    Comparator<String> byLength() {
        return Comparator.comparing(String::length);
    }

    Supplier<List<String>> supplier() {
        return ArrayList::new;
    }

    Function<Integer, Integer> adder(int k) {
        return v -> v + k + base;
    }

    Runnable anon(final String msg) {
        return new Runnable() {
            public void run() {
                System.out.println(msg + base);
            }
        };
    }

    Runnable block() {
        return () -> {
            for (int i = 0; i < 3; i++) {
                System.out.println(i);
            }
        };
    }

    class Inner {
        int get() {
            return base;
        }
    }

    static class Nested {
        String hello() {
            return "hi";
        }
    }
}
"#;

#[test]
fn lambdas() {
    for debug in [true, false] {
        let Some(text) = decompile(LAMBDAS, "Lambdas", debug) else {
            return;
        };
        check(
            &text,
            &[
                " -> ",
                "String::length",
                "ArrayList::new",
                "new Runnable() {",
                "public void run()",
                "class Inner",
                "class Nested",
            ],
        );
    }
}

const RECORDS: &str = r#"
public record Point(int x, String label, double w) {
    public Point {
        if (x < 0) {
            throw new IllegalArgumentException();
        }
    }
}
"#;

#[test]
fn records() {
    let Some(text) = decompile(RECORDS, "Point", true) else {
        return;
    };
    check(
        &text,
        &[
            "\"Point[x=\" + x + \", label=\" + label + \", w=\" + w + \"]\"",
            "Integer.hashCode(x)",
            "Objects.equals(label, ((Point) ",
            "Double.compare(w, ((Point) ",
        ],
    );
}

/// 很多 JDK 自带类：无 panic、无语法错误。设置 `JA_DECOMPILE_JARS`（逗号分隔的 jar 或
/// JDK 的 `lib/modules`）时用它们，否则用当前 JDK 的 `lib/modules` 里 `java/util/` 下的类。
#[test]
#[ignore]
fn robustness() {
    use classfile::{ClassContainer, JImageContainer, JarContainer};
    use rayon::prelude::*;

    let paths: Vec<PathBuf> = match std::env::var("JA_DECOMPILE_JARS") {
        Ok(v) => v.split(',').map(PathBuf::from).collect(),
        Err(_) => {
            let Some(home) = std::env::var_os("JAVA_HOME") else {
                return;
            };
            vec![Path::new(&home).join("lib").join("modules")]
        }
    };
    let prefix = std::env::var("JA_DECOMPILE_PREFIX").unwrap_or_default();
    let mut total = DecompileStats::default();
    for path in paths {
        let container: Box<dyn ClassContainer> = if path.file_name().is_some_and(|n| n == "modules")
        {
            Box::new(JImageContainer::open(path.clone()).expect("打不开 jimage"))
        } else {
            Box::new(JarContainer::new(path.clone()))
        };
        struct Env<'a>(&'a dyn ClassContainer, std::collections::HashSet<String>);
        impl ClassEnv for Env<'_> {
            fn class_bytes(&self, n: &str) -> Option<Vec<u8>> {
                self.0.read_class(n)
            }
            fn class_exists(&self, n: &str) -> bool {
                self.1.contains(n)
            }
        }
        let names: Vec<String> = container.class_names();
        let env = Env(container.as_ref(), names.iter().cloned().collect());
        let top: Vec<&String> = names
            .iter()
            .filter(|n| {
                !n.contains('$') && n.starts_with(prefix.as_str()) && !n.ends_with("module-info")
            })
            .collect();
        let start = std::time::Instant::now();
        let results: Vec<(DecompileStats, usize)> = top
            .par_iter()
            .map(|name| {
                let mut stats = DecompileStats::default();
                let Some(bytes) = env.class_bytes(name) else {
                    return (stats, 0);
                };
                let Ok(text) = decompile_class_text(&bytes, &env, &mut stats) else {
                    return (stats, 0);
                };
                let errors = syntax::ast::CompilationUnit::parse(&text).errors().len();
                if errors > 0 {
                    eprintln!("{name}: {errors} 个语法错误");
                }
                (stats, errors)
            })
            .collect();
        let elapsed = start.elapsed();
        let mut stats = DecompileStats::default();
        let mut errors = 0;
        for (s, e) in &results {
            stats.merge(s);
            errors += e;
        }
        eprintln!(
            "{}: {} 个类，{} 个方法，反编译成功 {} ({:.2}%)，用时 {:.2?}（每类 {:.2?}）",
            path.display(),
            stats.classes,
            stats.methods,
            stats.decompiled,
            100.0 * stats.decompiled as f64 / stats.methods.max(1) as f64,
            elapsed,
            elapsed / (top.len().max(1) as u32)
        );
        assert_eq!(errors, 0, "有语法错误");
        total.merge(&stats);
    }
    let mut failures: Vec<_> = total.failures.iter().collect();
    failures.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
    for (reason, n) in failures {
        eprintln!("  {n:>7}  {reason}");
    }
}
