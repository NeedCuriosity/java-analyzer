use test_fixture::fixture;

use crate::{completions, CompletionConfig, CompletionItem, CompletionItemKind};

fn complete(text: &str) -> Vec<CompletionItem> {
    let fx = fixture(text);
    let (file, _, offset) = fx.position();
    completions(&fx.db, file, offset, &CompletionConfig::default()).items
}

fn labels(items: &[CompletionItem]) -> Vec<&str> {
    items.iter().map(|item| item.label.as_str()).collect()
}

fn find<'a>(items: &'a [CompletionItem], label: &str) -> &'a CompletionItem {
    items
        .iter()
        .find(|item| item.label == label)
        .unwrap_or_else(|| panic!("没有候选 `{label}`，实际: {:?}", labels(items)))
}

#[test]
fn members_of_local_variable() {
    let items = complete(
        r#"
class Main {
    void f() {
        String s = "";
        s.$0
    }
}
"#,
    );
    let labels = labels(&items);
    assert!(labels.contains(&"length"), "{labels:?}");
    assert!(labels.contains(&"substring"), "{labels:?}");
    // 继承自 Object 的方法也在，但排在自己的方法后面
    let own = items.iter().position(|i| i.label == "length").unwrap();
    let inherited = items.iter().position(|i| i.label == "hashCode").unwrap();
    assert!(own < inherited);
}

#[test]
fn member_prefix_filters() {
    let items = complete(
        r#"
class Main {
    void f(String s) {
        s.sub$0
    }
}
"#,
    );
    assert!(
        items.iter().all(|i| i.label == "substring"),
        "{:?}",
        labels(&items)
    );
    assert_eq!(items.len(), 2, "两个重载");
}

#[test]
fn static_members_through_class_name() {
    let items = complete(
        r#"
class Main {
    void f() {
        Integer.$0
    }
}
"#,
    );
    let labels = labels(&items);
    assert!(labels.contains(&"MAX_VALUE"), "{labels:?}");
    assert!(labels.contains(&"parseInt"), "{labels:?}");
    assert!(
        !labels.contains(&"intValue"),
        "实例方法不应出现: {labels:?}"
    );
}

#[test]
fn locals_fields_and_methods_in_expression() {
    let items = complete(
        r#"
class Main {
    int count;
    void helper() {}
    void f(int param) {
        int local = 1;
        $0
    }
}
"#,
    );
    let labels = labels(&items);
    for expected in ["local", "param", "count", "helper", "f"] {
        assert!(labels.contains(&expected), "缺少 {expected}: {labels:?}");
    }
    let local = items.iter().position(|i| i.label == "local").unwrap();
    let field = items.iter().position(|i| i.label == "count").unwrap();
    assert!(local < field, "局部变量应排在字段前面");
}

#[test]
fn locals_declared_later_are_not_visible() {
    let items = complete(
        r#"
class Main {
    void f() {
        $0
        int later = 1;
    }
}
"#,
    );
    assert!(!labels(&items).contains(&"later"));
}

#[test]
fn unimported_type_adds_import() {
    let items = complete(
        r#"
//- /app/Main.java
package app;

class Main {
    void f() {
        ArrayL$0
    }
}
"#,
    );
    let item = find(&items, "ArrayList");
    assert_eq!(item.additional_edits.len(), 1);
    assert!(item.additional_edits[0]
        .new_text
        .contains("import java.util.ArrayList;"));
}

#[test]
fn java_lang_type_needs_no_import() {
    let items = complete(
        r#"
class Main {
    Stri$0
}
"#,
    );
    let item = find(&items, "String");
    assert!(item.additional_edits.is_empty());
}

#[test]
fn expected_type_ranks_matching_candidates_first() {
    let items = complete(
        r#"
class Main {
    void f() {
        String name = "";
        int number = 0;
        String other = n$0;
    }
}
"#,
    );
    let name = items.iter().position(|i| i.label == "name").unwrap();
    let number = items.iter().position(|i| i.label == "number").unwrap();
    assert!(name < number);
}

#[test]
fn import_path_segments() {
    let items = complete("import java.util.$0;\nclass Main {}\n");
    let labels = labels(&items);
    assert!(labels.contains(&"List"), "{labels:?}");
    assert!(labels.contains(&"function"), "{labels:?}");
}

#[test]
fn postfix_var_template() {
    let items = complete(
        r#"
class Main {
    void f(String s) {
        s.trim().var$0
    }
}
"#,
    );
    let item = find(&items, "var");
    assert_eq!(item.kind, CompletionItemKind::Snippet);
    assert!(
        item.insert_text.contains("= s.trim();"),
        "{}",
        item.insert_text
    );
    assert_eq!(item.additional_edits.len(), 1, "删除原来的接收者");
}

#[test]
fn override_method_in_class_body() {
    let items = complete(
        r#"
class Main {
    toS$0
}
"#,
    );
    let item = items
        .iter()
        .find(|i| i.label.starts_with("toString("))
        .unwrap_or_else(|| panic!("{:?}", labels(&items)));
    assert!(
        item.insert_text.starts_with("@Override"),
        "{}",
        item.insert_text
    );
    assert!(
        item.insert_text.contains("public String toString()"),
        "{}",
        item.insert_text
    );
}

#[test]
fn enum_constants_in_switch_case() {
    let items = complete(
        r#"
enum Color { RED, GREEN }
class Main {
    void f(Color c) {
        switch (c) {
            case $0
        }
    }
}
"#,
    );
    assert!(
        matches!(items[0].label.as_str(), "RED" | "GREEN"),
        "{:?}",
        labels(&items)
    );
    assert!(labels(&items).contains(&"RED"));
}

#[test]
fn no_completion_in_comments_and_strings() {
    assert!(complete("class Main { // foo$0\n}").is_empty());
    assert!(complete("class Main { String s = \"ab$0c\"; }").is_empty());
}

#[test]
fn keywords_at_statement_start() {
    let items = complete(
        r#"
class Main {
    void f() {
        ret$0
    }
}
"#,
    );
    let item = find(&items, "return");
    assert_eq!(item.kind, CompletionItemKind::Keyword);
}

#[test]
fn javadoc_block_and_inline_tags() {
    let items =
        complete("class Main {\n    /**\n     * @par$0\n     */\n    void f(int a) {}\n}\n");
    assert_eq!(labels(&items), ["param"]);

    let items = complete("class Main {\n    /** See {@li$0} */\n    void f() {}\n}\n");
    assert_eq!(labels(&items), ["link", "linkplain", "literal"]);

    // 行中间的 `@` 不是块标签
    let items = complete("class Main {\n    /** mail me @par$0 */\n    void f() {}\n}\n");
    assert!(items.is_empty());
}

#[test]
fn javadoc_param_names() {
    let items = complete(
        "class Main {\n    /**\n     * @param first one\n     * @param $0\n     */\n    <T> void f(int first, T second) {}\n}\n",
    );
    let labels = labels(&items);
    assert_eq!(labels, ["second", "<T>", "first"], "已写过说明的排最后");
}

#[test]
fn javadoc_block_tags_follow_the_declaration() {
    // 没有返回值的方法不给 `@return`
    let items = complete("class Main {\n    /**\n     * @$0\n     */\n    void f(int a) {}\n}\n");
    let shown = labels(&items);
    assert!(
        shown.contains(&"param") && shown.contains(&"throws"),
        "{shown:?}"
    );
    assert!(!shown.contains(&"return"), "{shown:?}");

    let items =
        complete("class Main {\n    /**\n     * @$0\n     */\n    int f() { return 0; }\n}\n");
    assert!(labels(&items).contains(&"return"));

    // 非泛型类上没有 `@param`、`@return`、`@throws`，有 `@author`
    let items = complete("/**\n * @$0\n */\nclass Main {}\n");
    let shown = labels(&items);
    assert!(shown.contains(&"author"), "{shown:?}");
    assert!(
        !shown.contains(&"param") && !shown.contains(&"return") && !shown.contains(&"throws"),
        "{shown:?}"
    );
}

#[test]
fn javadoc_param_names_only_count_this_comment() {
    // 别的注释里的 `@param first` 不算
    let items = complete(
        "class Main {\n    /** @param first x */\n    void g(int first) {}\n    /**\n     * @param $0\n     */\n    void f(int first, int second) {}\n}\n",
    );
    assert_eq!(labels(&items), ["first", "second"]);
}

#[test]
fn javadoc_link_types() {
    let items = complete(
        "import java.util.List;\nclass Main {\n    /** See {@link Li$0} */\n    void f() {}\n}\n",
    );
    let item = find(&items, "List");
    assert!(item.additional_edits.is_empty());

    // 没有导入的类型和代码里一样加 import
    let items = complete("class Main {\n    /** See {@link ArrayLi$0} */\n    void f() {}\n}\n");
    let item = find(&items, "ArrayList");
    assert_eq!(item.insert_text, "ArrayList");
    assert!(!item.additional_edits.is_empty(), "要加 import");

    // 限定名：包里的类
    let items = complete("class Main {\n    /** @see java.util.$0 */\n    void f() {}\n}\n");
    assert!(
        labels(&items).contains(&"ArrayList"),
        "{:?}",
        labels(&items)
    );
}

#[test]
fn javadoc_throws_prefers_declared_exceptions() {
    let items = complete(
        "import java.io.IOException;\nclass Main {\n    /**\n     * @throws $0\n     */\n    void f() throws IOException {}\n}\n",
    );
    assert_eq!(items.first().map(|i| i.label.as_str()), Some("IOException"));
}

#[test]
fn javadoc_link_members() {
    let items = complete(
        "class Main {\n    int count;\n    Main(int start) {}\n    void run(String name, int... rest) {}\n    /** Calls {@link #$0} */\n    void f() {}\n}\n",
    );
    let run = find(&items, "run");
    assert_eq!(run.insert_text, "run(String, int...)");
    assert_eq!(find(&items, "count").insert_text, "count");
    assert_eq!(find(&items, "Main").insert_text, "Main(int)");

    // 别的类的成员
    let items = complete("class Main {\n    /** {@link String#len$0} */\n    void f() {}\n}\n");
    assert_eq!(find(&items, "length").insert_text, "length()");

    // 类的注释里 `#` 指的是这个类自己的成员
    let items = complete("/** {@link #$0} */\nclass Main {\n    void run() {}\n}\n");
    assert!(labels(&items).contains(&"run"), "{:?}", labels(&items));

    // `@throws` 后面不能引用成员
    let items = complete("class Main {\n    /** @throws Foo#$0 */\n    void f() {}\n}\n");
    assert!(items.is_empty());
}

#[test]
fn annotations_follow_target() {
    // 类上没有 `@Override`
    let items = complete("@Ov$0\nclass Main {}\n");
    assert!(
        !labels(&items).contains(&"Override"),
        "{:?}",
        labels(&items)
    );

    // 方法上有，而且排在只能用在类型上的注解前面
    let items = complete("class Main {\n    @$0\n    public void f() {}\n}\n");
    let shown = labels(&items);
    let over = shown
        .iter()
        .position(|l| *l == "Override")
        .expect("Override");
    let functional = shown
        .iter()
        .position(|l| *l == "FunctionalInterface")
        .expect("类体里的注解不确定是给谁的，只排到后面");
    assert!(over < functional, "{shown:?}");

    // 形参上不给只能用在方法上的注解；源码里声明的 `@Target` 也算
    let items = complete(
        "import java.lang.annotation.*;\n@Target({ElementType.PARAMETER, ElementType.FIELD}) @interface Named {}\n@Target(ElementType.METHOD) @interface Action {}\nclass Main {\n    void f(@$0 String s) {}\n}\n",
    );
    let shown = labels(&items);
    assert!(shown.contains(&"Named"), "{shown:?}");
    assert!(
        !shown.contains(&"Action") && !shown.contains(&"Override"),
        "{shown:?}"
    );
}

#[test]
fn frequently_used_names_rank_higher() {
    let items = complete(
        r#"
class Main {
    void f() {
        int alpha = 1;
        int alpine = 2;
        use(alpine); use(alpine); use(alpine);
        al$0
    }
    void use(int x) {}
}
"#,
    );
    let alpha = items.iter().position(|i| i.label == "alpha").unwrap();
    let alpine = items.iter().position(|i| i.label == "alpine").unwrap();
    assert!(alpine < alpha);
}
