//! import 语句的编辑：让某个类在文件里能用简单名引用。

use base_db::{Db, SourceFile};
use hir::{class_data, resolve_simple_type, ClassId, TypeNs, TypeScope};
use syntax::{ast::AstNode, TextRange, TextSize};

use crate::TextEdit;

/// 想用简单名引用一个类时需要做什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportAction {
    /// 已经可见（同包、已 import、`java.lang`、嵌套 / 继承来的成员类型）
    None,
    /// 加一条 import
    Add(TextEdit),
    /// 简单名已被别的类型占用，只能写全限定名
    Qualify,
}

/// 类的可导入名，例如 `java.util.Map.Entry`。匿名类 / 局部类返回 `None`。
pub fn importable_name(db: &dyn Db, class: ClassId) -> Option<String> {
    let binary_name = class.binary_name(db);
    let simple = binary_name.rsplit(['/', '$']).next()?;
    if simple.is_empty() || simple.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some(binary_name.replace(['/', '$'], "."))
}

pub fn import_action(
    db: &dyn Db,
    file: SourceFile,
    scope: &TypeScope,
    class: ClassId,
) -> ImportAction {
    let Some(data) = class_data(db, class) else {
        return ImportAction::Qualify;
    };
    match resolve_simple_type(db, scope, &data.name) {
        Some(TypeNs::Class(found)) if found == class => ImportAction::None,
        Some(_) => ImportAction::Qualify,
        None => match importable_name(db, class) {
            Some(name) => ImportAction::Add(insert_import(db, file, &name)),
            None => ImportAction::Qualify,
        },
    }
}

/// 生成插入 `import <qualified_name>;` 的编辑：按 import 布局放进所属的组，组内按字母序。
pub fn insert_import(db: &dyn Db, file: SourceFile, qualified_name: &str) -> TextEdit {
    let cu = base_db::parse(db, file).tree();
    let line = format!("import {qualified_name};");
    let layout = import_layout(db);
    let key = (
        layout.group_of(qualified_name, false),
        qualified_name.to_owned(),
    );

    // 已有的 import（模块导入排在所有组前面）
    let existing: Vec<((usize, String), TextRange)> = cu
        .imports()
        .filter_map(|import| {
            let name = import.name()?.text();
            let group = if import.is_module() {
                0
            } else {
                let path = if import.is_on_demand() {
                    format!("{name}.*")
                } else {
                    name.to_string()
                };
                return Some((
                    (layout.group_of(&path, import.is_static()) + 1, path),
                    import.syntax().text_range(),
                ));
            };
            Some(((group, name.to_string()), import.syntax().text_range()))
        })
        .collect();
    let key = (key.0 + 1, key.1);
    let separator = |a: usize, b: usize| {
        let blank = if a == 0 || b == 0 {
            a != b
        } else {
            layout.blank_between(a - 1, b - 1)
        };
        if blank {
            "\n\n"
        } else {
            "\n"
        }
    };
    if let Some(pos) = existing.iter().position(|(k, _)| *k > key) {
        let (next_key, next_range) = &existing[pos];
        return TextEdit::insert(
            next_range.start(),
            format!("{line}{}", separator(key.0, next_key.0)),
        );
    }
    if let Some((last_key, last_range)) = existing.last() {
        return TextEdit::insert(
            last_range.end(),
            format!("{}{line}", separator(last_key.0, key.0)),
        );
    }
    if let Some(package) = cu.package() {
        return TextEdit::insert(package.syntax().text_range().end(), format!("\n\n{line}"));
    }
    TextEdit::insert(TextSize::from(0), format!("{line}\n\n"))
}

// ---------------------------------------------------------------------------
// import 的分组布局
// ---------------------------------------------------------------------------

/// 布局里的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
enum LayoutEntry {
    /// 组之间空一行（`|`）
    Blank,
    /// 一组 import：`java.**`（含子包）、`java.*`（只有这个包）、`*`（其余全部），
    /// 前面带 `$` 的是静态 import
    Group {
        is_static: bool,
        package: String,
        subpackages: bool,
    },
}

/// import 的分组顺序。默认：普通 import 一组，空一行，静态 import 一组；
/// `.editorconfig` 里的 `ij_java_imports_layout` 可以改（`*,|,javax.**,java.**,|,$*`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportLayout {
    entries: Vec<LayoutEntry>,
}

impl Default for ImportLayout {
    fn default() -> Self {
        ImportLayout::parse("*,|,$*").expect("默认布局")
    }
}

impl ImportLayout {
    /// 解析 `ij_java_imports_layout` 的值。
    pub fn parse(value: &str) -> Option<ImportLayout> {
        let mut entries = Vec::new();
        for raw in value.split(',') {
            let item = raw.trim();
            if item.is_empty() {
                continue;
            }
            if item == "|" {
                entries.push(LayoutEntry::Blank);
                continue;
            }
            let (is_static, rest) = match item.strip_prefix('$') {
                Some(rest) => (true, rest),
                None => (false, item),
            };
            let (package, subpackages) = if rest == "*" {
                (String::new(), true)
            } else if let Some(p) = rest.strip_suffix(".**") {
                (p.to_owned(), true)
            } else if let Some(p) = rest.strip_suffix(".*") {
                (p.to_owned(), false)
            } else {
                (rest.to_owned(), true)
            };
            entries.push(LayoutEntry::Group {
                is_static,
                package,
                subpackages,
            });
        }
        let has_group = entries
            .iter()
            .any(|e| matches!(e, LayoutEntry::Group { .. }));
        if !has_group {
            return None;
        }
        // 没有兜底的 `*` / `$*` 时补在最后，保证每条 import 都有归属
        for is_static in [false, true] {
            let covered = entries.iter().any(|e| {
                matches!(e, LayoutEntry::Group { is_static: s, package, .. } if *s == is_static && package.is_empty())
            });
            if !covered {
                entries.push(LayoutEntry::Group {
                    is_static,
                    package: String::new(),
                    subpackages: true,
                });
            }
        }
        Some(ImportLayout { entries })
    }

    /// `name`（`java.util.List`、`java.util.*`、静态 import 的 `a.B.member`）属于哪一组：
    /// 匹配的组里包名最长的那个。
    pub fn group_of(&self, name: &str, is_static: bool) -> usize {
        let package = name.rsplit_once('.').map_or("", |(p, _)| p);
        let mut best: Option<(usize, usize)> = None;
        for (idx, entry) in self.entries.iter().enumerate() {
            let LayoutEntry::Group {
                is_static: s,
                package: p,
                subpackages,
            } = entry
            else {
                continue;
            };
            if *s != is_static {
                continue;
            }
            let matches = if p.is_empty() {
                true
            } else if *subpackages {
                package == p || package.starts_with(&format!("{p}."))
            } else {
                package == p
            };
            if matches && best.is_none_or(|(_, len)| p.len() > len) {
                best = Some((idx, p.len()));
            }
        }
        best.map_or(self.entries.len(), |(idx, _)| idx)
    }

    /// 两组之间要不要空行（布局里两组之间有 `|`）。
    pub fn blank_between(&self, a: usize, b: usize) -> bool {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        a != b
            && self.entries[lo.min(self.entries.len())..hi.min(self.entries.len())]
                .contains(&LayoutEntry::Blank)
    }

    /// 按布局排好一批 import 语句（`(全名, 是否静态, 原文)`）：组内按全名排序，
    /// 需要的地方空一行。模块导入放在最前面。
    pub fn arrange(&self, modules: Vec<String>, imports: Vec<(String, bool, String)>) -> String {
        let mut keyed: Vec<(usize, String, String)> = imports
            .into_iter()
            .map(|(name, is_static, text)| (self.group_of(&name, is_static), name, text))
            .collect();
        keyed.sort();
        keyed.dedup_by(|a, b| a.2 == b.2);
        let mut out = String::new();
        let mut modules = modules;
        modules.sort();
        modules.dedup();
        for module in &modules {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(module);
        }
        let mut prev: Option<usize> = None;
        for (group, _, text) in keyed {
            match prev {
                None if !out.is_empty() => out.push_str("\n\n"),
                None => {}
                Some(p) if self.blank_between(p, group) => out.push_str("\n\n"),
                Some(_) => out.push('\n'),
            }
            out.push_str(&text);
            prev = Some(group);
        }
        out
    }
}

/// 一个 `.editorconfig` 里对 Java 文件生效的 `ij_java_imports_layout`。
fn editorconfig_import_layout(text: &str) -> Option<String> {
    let mut applies = false;
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            applies = section_matches_java(header);
            continue;
        }
        if !applies {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim().eq_ignore_ascii_case("ij_java_imports_layout") {
                found = Some(value.trim().to_owned());
            }
        }
    }
    found
}

/// 节的模式是否匹配 Java 源文件（只看文件名部分：`*`、`*.java`、`{*.java,*.kt}`、`**/*.java`）。
fn section_matches_java(pattern: &str) -> bool {
    let file_part = pattern.rsplit('/').next().unwrap_or(pattern);
    expand_braces(file_part)
        .iter()
        .any(|alt| glob_matches(alt, "Example.java"))
}

fn expand_braces(pattern: &str) -> Vec<String> {
    let (Some(open), Some(close)) = (pattern.find('{'), pattern.find('}')) else {
        return vec![pattern.to_owned()];
    };
    if close < open {
        return vec![pattern.to_owned()];
    }
    let (head, rest) = pattern.split_at(open);
    let inner = &rest[1..close - open];
    let tail = &rest[close - open + 1..];
    inner
        .split(',')
        .flat_map(|alt| expand_braces(&format!("{head}{alt}{tail}")))
        .collect()
}

/// `*`（包括 `**`）匹配任意多个字符，`?` 匹配一个。
fn glob_matches(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            while pi < p.len() && p[pi] == '*' {
                pi += 1;
            }
            star = Some(pi);
            mark = ni;
        } else if let Some(s) = star {
            pi = s;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// 工作区的 import 布局：`.editorconfig` 里对 Java 文件生效的 `ij_java_imports_layout`
/// （外层目录的先读，里层的覆盖），没有时用默认布局。
#[salsa::tracked(returns(ref))]
pub fn import_layout(db: &dyn Db) -> ImportLayout {
    let mut files: Vec<&base_db::ResourceFile> = base_db::workspace(db)
        .resources(db)
        .iter()
        .filter(|r| r.name == ".editorconfig")
        .collect();
    files.sort_by_key(|r| r.file_id);
    // 里层目录的 `.editorconfig` 覆盖外层的：从后往前取第一个有效的
    files
        .into_iter()
        .rev()
        .filter_map(|f| editorconfig_import_layout(&f.text))
        .find_map(|value| ImportLayout::parse(&value))
        .unwrap_or_default()
}

/// `.editorconfig` 里对 Java 文件生效的一个整数设置（`ij_java_class_count_to_use_import_on_demand`）。
fn editorconfig_number(text: &str, property: &str) -> Option<usize> {
    let mut applies = false;
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            applies = section_matches_java(header);
            continue;
        }
        if !applies {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim().eq_ignore_ascii_case(property) {
                found = value.trim().parse().ok();
            }
        }
    }
    found
}

/// 同一个包里有多少个类被 import 时合并成 `包.*`（`ij_java_class_count_to_use_import_on_demand`）。
/// 没有配置时为 `None`：整理 import 不合并也不展开通配符 import。
#[salsa::tracked]
pub fn import_on_demand_threshold(db: &dyn Db) -> Option<usize> {
    let mut files: Vec<&base_db::ResourceFile> = base_db::workspace(db)
        .resources(db)
        .iter()
        .filter(|r| r.name == ".editorconfig")
        .collect();
    files.sort_by_key(|r| r.file_id);
    files
        .into_iter()
        .rev()
        .find_map(|f| editorconfig_number(&f.text, "ij_java_class_count_to_use_import_on_demand"))
}

/// 按阈值合并 / 展开普通（非静态）import：同一个包里用到的类达到 `threshold` 个时合并成
/// `包.*`，已有的 `包.*` 用到的类不到 `threshold` 个时展开成逐个的 import。
///
/// `entries` 是 `(全名, 是否静态, 原文)`；`used` 是文件里出现过的标识符。合并 / 展开都保证
/// 每个用到的简单名解析到的类不变：同包的类、`java.lang` 和其它通配符 import 里的同名类
/// 会让对应的名字保留（或补上）单独的 import。
pub fn apply_on_demand_threshold(
    db: &dyn Db,
    file: SourceFile,
    entries: Vec<(String, bool, String)>,
    used: &rustc_hash::FxHashSet<String>,
    threshold: usize,
) -> Vec<(String, bool, String)> {
    use rustc_hash::{FxHashMap, FxHashSet};
    let threshold = threshold.max(1);
    let cu = base_db::parse(db, file).tree();
    let own_package = cu
        .package()
        .and_then(|p| p.name())
        .map(|n| n.text().replace('.', "/"))
        .unwrap_or_default();
    let scope = TypeScope::file(file);
    let classes_of = |package: &str| -> FxHashSet<String> {
        hir::package_classes(db, package)
            .iter()
            .map(|n| n.to_string())
            .collect()
    };
    let is_package = |dotted: &str| hir::package_exists(db, &dotted.replace('.', "/"));

    // 普通 import 按包分开：单个类的、整个包的；其余（静态、嵌套类、模块）原样保留
    let mut kept: Vec<(String, bool, String)> = Vec::new();
    let mut singles: FxHashMap<String, Vec<String>> = FxHashMap::default();
    let mut stars: Vec<String> = Vec::new();
    for (name, is_static, text) in entries {
        if is_static {
            kept.push((name, is_static, text));
            continue;
        }
        if let Some(package) = name.strip_suffix(".*") {
            if is_package(package) {
                stars.push(package.to_owned());
            } else {
                kept.push((name, is_static, text));
            }
            continue;
        }
        match name.rsplit_once('.') {
            Some((package, simple)) if is_package(package) => {
                singles
                    .entry(package.to_owned())
                    .or_default()
                    .push(simple.to_owned());
            }
            _ => kept.push((name, is_static, text)),
        }
    }
    let resolves_to = |simple: &str| -> Option<String> {
        match resolve_simple_type(db, &scope, simple)? {
            TypeNs::Class(class) => Some(class.binary_name(db).to_string()),
            TypeNs::TypeParam(_) => None,
        }
    };

    // 展开用到的类不够多的通配符 import
    let mut final_stars: Vec<String> = Vec::new();
    for package in stars {
        let slash = package.replace('.', "/");
        let mut from_here: Vec<String> = classes_of(&slash)
            .into_iter()
            .filter(|simple| used.contains(simple))
            .filter(|simple| resolves_to(simple).as_deref() == Some(&format!("{slash}/{simple}")))
            .collect();
        if from_here.len() >= threshold {
            final_stars.push(package);
        } else {
            from_here.sort();
            singles.entry(package).or_default().extend(from_here);
        }
    }

    // 合并单个类的 import 达到阈值的包（本包和 java.lang 不用 import）
    let candidates: Vec<String> = singles
        .iter()
        .filter(|(package, names)| {
            names.len() >= threshold
                && package.replace('.', "/") != own_package
                && package.as_str() != "java.lang"
        })
        .map(|(package, _)| package.clone())
        .collect();
    final_stars.extend(candidates.iter().cloned());
    final_stars.sort();
    final_stars.dedup();

    let own_classes = classes_of(&own_package);
    let star_classes: FxHashMap<String, FxHashSet<String>> = final_stars
        .iter()
        .chain(std::iter::once(&"java.lang".to_owned()))
        .map(|p| (p.clone(), classes_of(&p.replace('.', "/"))))
        .collect();
    let single_names: FxHashSet<String> = singles.values().flatten().cloned().collect();
    let mut extra: Vec<String> = Vec::new();
    for package in &candidates {
        let Some(names) = singles.get_mut(package) else {
            continue;
        };
        // 合并后还要单独 import 的：同包或别的通配符包里也有同名的类
        names.retain(|simple| {
            own_classes.contains(simple)
                || star_classes
                    .iter()
                    .any(|(other, classes)| other != package && classes.contains(simple))
        });
        // 原来经由别的通配符包或 java.lang 解析的名字，合并后会和这个包里的同名类冲突
        let here = &star_classes[package];
        for simple in used.iter().filter(|s| here.contains(*s)) {
            if single_names.contains(simple) || own_classes.contains(simple) {
                continue;
            }
            let Some(binary) = resolves_to(simple) else {
                continue;
            };
            let (from, _) = binary.rsplit_once('/').unwrap_or(("", ""));
            let from_dotted = from.replace('/', ".");
            if from_dotted != *package
                && !binary.contains('$')
                && (star_classes.contains_key(&from_dotted) || from_dotted == "java.lang")
            {
                extra.push(binary.replace('/', "."));
            }
        }
    }

    let mut res = kept;
    for package in &final_stars {
        res.push((
            format!("{package}.*"),
            false,
            format!("import {package}.*;"),
        ));
    }
    for (package, names) in singles {
        for simple in names {
            let name = format!("{package}.{simple}");
            let text = format!("import {name};");
            res.push((name, false, text));
        }
    }
    for name in extra {
        let text = format!("import {name};");
        res.push((name, false, text));
    }
    res
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn groups_by_most_specific_entry() {
        let layout = ImportLayout::parse("*,|,javax.**,java.**,|,$*").unwrap();
        let g = |name: &str, s: bool| layout.group_of(name, s);
        assert_eq!(g("org.acme.Foo", false), 0);
        assert_eq!(g("javax.inject.Inject", false), 2);
        assert_eq!(g("java.util.List", false), 3);
        assert_eq!(g("java.util.Map.Entry", false), 3);
        assert_eq!(g("org.junit.Assert.assertEquals", true), 5);
        assert!(!layout.blank_between(2, 3));
        assert!(layout.blank_between(0, 2));
        let text = layout.arrange(
            vec![],
            vec![
                (
                    "java.util.List".into(),
                    false,
                    "import java.util.List;".into(),
                ),
                ("org.acme.Foo".into(), false, "import org.acme.Foo;".into()),
                (
                    "javax.inject.Inject".into(),
                    false,
                    "import javax.inject.Inject;".into(),
                ),
                ("a.B.c".into(), true, "import static a.B.c;".into()),
            ],
        );
        assert_eq!(
            text,
            "import org.acme.Foo;\n\nimport javax.inject.Inject;\nimport java.util.List;\n\nimport static a.B.c;"
        );
    }

    #[test]
    fn statics_first_and_editorconfig_sections() {
        let layout = ImportLayout::parse("$*,|,*").unwrap();
        assert!(layout.group_of("a.B.c", true) < layout.group_of("java.util.List", false));
        let config = "root = true\n[*]\nindent_style = space\n[*.{java,kt}]\nij_java_imports_layout = $*,|,*\n[*.md]\nij_java_imports_layout = *\n";
        assert_eq!(
            editorconfig_import_layout(config).as_deref(),
            Some("$*,|,*")
        );
        assert!(section_matches_java("**/*.java"));
        assert!(section_matches_java("*"));
        assert!(!section_matches_java("*.kt"));
        assert_eq!(
            ImportLayout::default(),
            ImportLayout::parse("*,|,$*").unwrap()
        );
    }
}
