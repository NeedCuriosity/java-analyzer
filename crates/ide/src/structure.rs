//! 纯语法级的功能：文件结构、折叠、扩选、括号匹配。不需要任何语义信息，索引完成前即可用。

use ide_db::SymbolKind;
use syntax::{
    ast::{self, AstNode},
    Direction, NodeOrToken, SyntaxKind, SyntaxNode, TextRange, TextSize,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructureNode {
    pub name: String,
    /// 方法的形参表与返回类型、字段的类型
    pub detail: Option<String>,
    pub kind: SymbolKind,
    pub full_range: TextRange,
    pub focus_range: TextRange,
    pub deprecated: bool,
    pub children: Vec<StructureNode>,
}

fn normalized_text(node: &SyntaxNode) -> String {
    let text = node.text().to_string();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_deprecated(node: &SyntaxNode) -> bool {
    node.children()
        .find_map(ast::ModifierList::cast)
        .is_some_and(|mods| {
            mods.annotations().any(|a| {
                a.name()
                    .is_some_and(|n| n.segments().last().is_some_and(|s| s == "Deprecated"))
            })
        })
}

fn type_decl_node(decl: &ast::TypeDecl) -> Option<StructureNode> {
    let name = decl.name()?;
    let kind = match decl {
        ast::TypeDecl::ClassDecl(_) => SymbolKind::Class,
        ast::TypeDecl::InterfaceDecl(_) => SymbolKind::Interface,
        ast::TypeDecl::EnumDecl(_) => SymbolKind::Enum,
        ast::TypeDecl::RecordDecl(_) => SymbolKind::Record,
        ast::TypeDecl::AnnotationTypeDecl(_) => SymbolKind::Annotation,
    };
    let mut children = Vec::new();
    if let Some(components) = decl.record_components() {
        for param in components.params() {
            let Some(param_name) = param.name() else {
                continue;
            };
            children.push(StructureNode {
                name: param_name.text().to_string(),
                detail: param.ty().map(|ty| normalized_text(ty.syntax())),
                kind: SymbolKind::Field,
                full_range: param.syntax().text_range(),
                focus_range: param_name.syntax().text_range(),
                deprecated: false,
                children: Vec::new(),
            });
        }
    }
    if let Some(body) = decl.body() {
        members(body.syntax(), &mut children);
    }
    Some(StructureNode {
        name: name.text().to_string(),
        detail: None,
        kind,
        full_range: decl.syntax().text_range(),
        focus_range: name.syntax().text_range(),
        deprecated: is_deprecated(decl.syntax()),
        children,
    })
}

fn callable_node(
    node: &SyntaxNode,
    name: Option<ast::Name>,
    params: Option<ast::ParamList>,
    ret: Option<ast::Type>,
    kind: SymbolKind,
) -> Option<StructureNode> {
    let name = name?;
    let mut detail = params
        .map(|p| normalized_text(p.syntax()))
        .unwrap_or_else(|| "()".to_owned());
    if let Some(ret) = ret {
        detail.push_str(": ");
        detail.push_str(&normalized_text(ret.syntax()));
    }
    Some(StructureNode {
        name: name.text().to_string(),
        detail: Some(detail),
        kind,
        full_range: node.text_range(),
        focus_range: name.syntax().text_range(),
        deprecated: is_deprecated(node),
        children: Vec::new(),
    })
}

fn members(body: &SyntaxNode, out: &mut Vec<StructureNode>) {
    for child in body.children() {
        match child.kind() {
            SyntaxKind::ENUM_CONSTANT => {
                let Some(constant) = ast::EnumConstant::cast(child.clone()) else {
                    continue;
                };
                let Some(name) = constant.name() else {
                    continue;
                };
                out.push(StructureNode {
                    name: name.text().to_string(),
                    detail: None,
                    kind: SymbolKind::EnumConstant,
                    full_range: child.text_range(),
                    focus_range: name.syntax().text_range(),
                    deprecated: false,
                    children: Vec::new(),
                });
            }
            SyntaxKind::FIELD_DECL => {
                let Some(field) = ast::FieldDecl::cast(child.clone()) else {
                    continue;
                };
                let ty = field.ty().map(|ty| normalized_text(ty.syntax()));
                let deprecated = is_deprecated(&child);
                for declarator in field.declarators() {
                    let Some(name) = declarator.name() else {
                        continue;
                    };
                    out.push(StructureNode {
                        name: name.text().to_string(),
                        detail: ty.clone(),
                        kind: SymbolKind::Field,
                        full_range: child.text_range(),
                        focus_range: name.syntax().text_range(),
                        deprecated,
                        children: Vec::new(),
                    });
                }
            }
            SyntaxKind::METHOD_DECL => {
                let Some(method) = ast::MethodDecl::cast(child.clone()) else {
                    continue;
                };
                out.extend(callable_node(
                    &child,
                    method.name(),
                    method.param_list(),
                    method.return_type(),
                    SymbolKind::Method,
                ));
            }
            SyntaxKind::CONSTRUCTOR_DECL => {
                let Some(ctor) = ast::ConstructorDecl::cast(child.clone()) else {
                    continue;
                };
                out.extend(callable_node(
                    &child,
                    ctor.name(),
                    ctor.param_list(),
                    None,
                    SymbolKind::Constructor,
                ));
            }
            _ => {
                if let Some(decl) = ast::TypeDecl::cast(child) {
                    out.extend(type_decl_node(&decl));
                }
            }
        }
    }
}

pub(crate) fn file_structure(cu: &ast::CompilationUnit) -> Vec<StructureNode> {
    cu.type_decls()
        .filter_map(|decl| type_decl_node(&decl))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FoldKind {
    Comment,
    Imports,
    Region,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fold {
    pub range: TextRange,
    pub kind: FoldKind,
}

fn is_multiline(node: &SyntaxNode) -> bool {
    node.text().contains_char('\n')
}

pub(crate) fn folding_ranges(root: &SyntaxNode) -> Vec<Fold> {
    let mut res = Vec::new();
    let mut seen_import_group = false;
    let mut line_comment_group: Option<TextRange> = None;

    for element in root.descendants_with_tokens() {
        match element {
            NodeOrToken::Node(node) => match node.kind() {
                SyntaxKind::CLASS_BODY
                | SyntaxKind::BLOCK
                | SyntaxKind::SWITCH_BLOCK
                | SyntaxKind::ARRAY_INITIALIZER
                | SyntaxKind::ARG_LIST
                | SyntaxKind::PARAM_LIST
                | SyntaxKind::ANNOTATION_ARG_LIST => {
                    if is_multiline(&node) {
                        res.push(Fold {
                            range: node.text_range(),
                            kind: FoldKind::Region,
                        });
                    }
                }
                SyntaxKind::IMPORT_DECL if !seen_import_group => {
                    seen_import_group = true;
                    let last = node
                        .siblings(Direction::Next)
                        .filter(|n| n.kind() == SyntaxKind::IMPORT_DECL)
                        .last()
                        .unwrap_or_else(|| node.clone());
                    if last != node {
                        let range =
                            TextRange::new(node.text_range().start(), last.text_range().end());
                        res.push(Fold {
                            range,
                            kind: FoldKind::Imports,
                        });
                    }
                }
                _ => {}
            },
            NodeOrToken::Token(token) => match token.kind() {
                SyntaxKind::BLOCK_COMMENT | SyntaxKind::DOC_COMMENT => {
                    if token.text().contains('\n') {
                        res.push(Fold {
                            range: token.text_range(),
                            kind: FoldKind::Comment,
                        });
                    }
                }
                SyntaxKind::LINE_COMMENT => {
                    line_comment_group = Some(match line_comment_group {
                        Some(group) => group.cover(token.text_range()),
                        None => token.text_range(),
                    });
                }
                SyntaxKind::WHITESPACE => {
                    // 空行打断连续的行注释
                    if token.text().matches('\n').count() > 1 {
                        flush_comment_group(&mut line_comment_group, root, &mut res);
                    }
                }
                _ => flush_comment_group(&mut line_comment_group, root, &mut res),
            },
        }
    }
    flush_comment_group(&mut line_comment_group, root, &mut res);
    res.sort_by_key(|fold| (fold.range.start(), std::cmp::Reverse(fold.range.end())));
    res
}

fn flush_comment_group(group: &mut Option<TextRange>, root: &SyntaxNode, out: &mut Vec<Fold>) {
    if let Some(range) = group.take() {
        let multiline = root.text().slice(range).contains_char('\n');
        if multiline {
            out.push(Fold {
                range,
                kind: FoldKind::Comment,
            });
        }
    }
}

pub(crate) fn selection_ranges(root: &SyntaxNode, offset: TextSize) -> Vec<TextRange> {
    let Some(token) = syntax::algo::pick_ident_token(root, offset) else {
        return Vec::new();
    };
    let mut res = vec![token.text_range()];
    for node in token.parent_ancestors() {
        let range = node.text_range();
        if res.last() != Some(&range) {
            res.push(range);
        }
    }
    res
}

/// 光标紧邻一个括号时，返回与它配对的括号的位置。
pub(crate) fn matching_brace(root: &SyntaxNode, offset: TextSize) -> Option<TextSize> {
    const PAIRS: &[(SyntaxKind, SyntaxKind)] = &[
        (SyntaxKind::L_CURLY, SyntaxKind::R_CURLY),
        (SyntaxKind::L_PAREN, SyntaxKind::R_PAREN),
        (SyntaxKind::L_BRACK, SyntaxKind::R_BRACK),
    ];
    let token = root.token_at_offset(offset).find(|t| {
        PAIRS
            .iter()
            .any(|(open, close)| t.kind() == *open || t.kind() == *close)
    })?;
    let parent = token.parent()?;
    let (open, close) = PAIRS
        .iter()
        .find(|(o, c)| token.kind() == *o || token.kind() == *c)?;
    let is_open = token.kind() == *open;
    let wanted = if is_open { *close } else { *open };
    // 配对的括号是同一个父节点下、嵌套深度归零处的那个
    let tokens: Vec<_> = parent
        .children_with_tokens()
        .filter_map(|element| element.into_token())
        .collect();
    let idx = tokens.iter().position(|t| *t == token)?;
    let mut depth = 0i32;
    let iter: Box<dyn Iterator<Item = &syntax::SyntaxToken>> = if is_open {
        Box::new(tokens[idx + 1..].iter())
    } else {
        Box::new(tokens[..idx].iter().rev())
    };
    for candidate in iter {
        if candidate.kind() == token.kind() {
            depth += 1;
        } else if candidate.kind() == wanted {
            if depth == 0 {
                return Some(candidate.text_range().start());
            }
            depth -= 1;
        }
    }
    None
}
