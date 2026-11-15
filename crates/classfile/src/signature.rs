//! 描述符与泛型签名（JVMS §4.3、§4.7.9.1）的解析。

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BaseType {
    Boolean,
    Byte,
    Short,
    Char,
    Int,
    Long,
    Float,
    Double,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JType {
    Base(BaseType),
    /// 类类型。`name` 是二进制名（嵌套类用 `$` 连接），`args` 是最内层那一段的类型实参。
    Class {
        name: String,
        args: Vec<TypeArg>,
    },
    TypeVar(String),
    Array(Box<JType>),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeArg {
    /// `*`，即 `?`
    Unbounded,
    /// `+T`，即 `? extends T`
    Extends(JType),
    /// `-T`，即 `? super T`
    Super(JType),
    Exact(JType),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeParamSig {
    pub name: String,
    pub bounds: Vec<JType>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClassSig {
    pub type_params: Vec<TypeParamSig>,
    pub super_class: JType,
    pub interfaces: Vec<JType>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MethodSig {
    pub type_params: Vec<TypeParamSig>,
    pub params: Vec<JType>,
    /// `None` 表示 `void`
    pub ret: Option<JType>,
    pub throws: Vec<JType>,
}

struct Cursor<'a> {
    s: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(s: &'a str) -> Self {
        Cursor {
            s: s.as_bytes(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek()?;
        self.pos += 1;
        Some(b)
    }

    fn eat(&mut self, b: u8) -> bool {
        if self.peek() == Some(b) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.s.len()
    }

    /// 读到 `stops` 中任意一个字节之前。
    fn ident(&mut self, stops: &[u8]) -> Option<String> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if stops.contains(&b) {
                break;
            }
            self.pos += 1;
        }
        if self.pos == start {
            return None;
        }
        std::str::from_utf8(&self.s[start..self.pos])
            .ok()
            .map(str::to_owned)
    }

    fn type_params(&mut self) -> Option<Vec<TypeParamSig>> {
        let mut res = Vec::new();
        if !self.eat(b'<') {
            return Some(res);
        }
        while !self.eat(b'>') {
            let name = self.ident(b":")?;
            let mut bounds = Vec::new();
            // 类约束可以为空：`T::Ljava/lang/Comparable;`
            while self.eat(b':') {
                if matches!(self.peek(), Some(b':')) {
                    continue;
                }
                bounds.push(self.field_type()?);
            }
            res.push(TypeParamSig { name, bounds });
        }
        Some(res)
    }

    fn field_type(&mut self) -> Option<JType> {
        let b = self.bump()?;
        Some(match b {
            b'Z' => JType::Base(BaseType::Boolean),
            b'B' => JType::Base(BaseType::Byte),
            b'S' => JType::Base(BaseType::Short),
            b'C' => JType::Base(BaseType::Char),
            b'I' => JType::Base(BaseType::Int),
            b'J' => JType::Base(BaseType::Long),
            b'F' => JType::Base(BaseType::Float),
            b'D' => JType::Base(BaseType::Double),
            b'[' => JType::Array(Box::new(self.field_type()?)),
            b'T' => {
                let name = self.ident(b";")?;
                self.eat(b';');
                JType::TypeVar(name)
            }
            b'L' => self.class_type_rest()?,
            _ => return None,
        })
    }

    /// 已经吃掉了 `L`。
    fn class_type_rest(&mut self) -> Option<JType> {
        let mut name = self.ident(b"<;.")?;
        let mut args = self.type_args()?;
        // 内部类段：`Lpkg/Outer<TA;>.Inner<TB;>;`
        while self.eat(b'.') {
            let segment = self.ident(b"<;.")?;
            name.push('$');
            name.push_str(&segment);
            args = self.type_args()?;
        }
        if !self.eat(b';') {
            return None;
        }
        Some(JType::Class { name, args })
    }

    fn type_args(&mut self) -> Option<Vec<TypeArg>> {
        let mut res = Vec::new();
        if !self.eat(b'<') {
            return Some(res);
        }
        while !self.eat(b'>') {
            let arg = match self.peek()? {
                b'*' => {
                    self.pos += 1;
                    TypeArg::Unbounded
                }
                b'+' => {
                    self.pos += 1;
                    TypeArg::Extends(self.field_type()?)
                }
                b'-' => {
                    self.pos += 1;
                    TypeArg::Super(self.field_type()?)
                }
                _ => TypeArg::Exact(self.field_type()?),
            };
            res.push(arg);
        }
        Some(res)
    }

    fn return_type(&mut self) -> Option<Option<JType>> {
        if self.eat(b'V') {
            Some(None)
        } else {
            self.field_type().map(Some)
        }
    }
}

/// 字段描述符或字段泛型签名，例如 `I`、`Ljava/util/List<TT;>;`。
pub fn parse_field_type(s: &str) -> Option<JType> {
    let mut c = Cursor::new(s);
    let ty = c.field_type()?;
    c.at_end().then_some(ty)
}

/// 方法描述符或方法泛型签名。
pub fn parse_method_sig(s: &str) -> Option<MethodSig> {
    let mut c = Cursor::new(s);
    let type_params = c.type_params()?;
    if !c.eat(b'(') {
        return None;
    }
    let mut params = Vec::new();
    while !c.eat(b')') {
        params.push(c.field_type()?);
    }
    let ret = c.return_type()?;
    let mut throws = Vec::new();
    while c.eat(b'^') {
        throws.push(c.field_type()?);
    }
    c.at_end().then_some(MethodSig {
        type_params,
        params,
        ret,
        throws,
    })
}

/// 类的泛型签名。
pub fn parse_class_sig(s: &str) -> Option<ClassSig> {
    let mut c = Cursor::new(s);
    let type_params = c.type_params()?;
    let super_class = c.field_type()?;
    let mut interfaces = Vec::new();
    while !c.at_end() {
        interfaces.push(c.field_type()?);
    }
    Some(ClassSig {
        type_params,
        super_class,
        interfaces,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(name: &str) -> JType {
        JType::Class {
            name: name.into(),
            args: vec![],
        }
    }

    #[test]
    fn descriptors() {
        assert_eq!(parse_field_type("I"), Some(JType::Base(BaseType::Int)));
        assert_eq!(
            parse_field_type("[[Ljava/lang/String;"),
            Some(JType::Array(Box::new(JType::Array(Box::new(class(
                "java/lang/String"
            ))))))
        );
        let m = parse_method_sig("(IJLjava/lang/Object;)V").unwrap();
        assert_eq!(m.params.len(), 3);
        assert_eq!(m.ret, None);
        assert!(parse_field_type("Ljava/lang/String").is_none());
    }

    #[test]
    fn generic_method() {
        // <T:Ljava/lang/Object;R::Ljava/lang/Comparable<TR;>;>(Ljava/util/List<+TT;>;[TR;)TT;^TE;
        let m = parse_method_sig(
            "<T:Ljava/lang/Object;R::Ljava/lang/Comparable<TR;>;>(Ljava/util/List<+TT;>;[TR;)TT;^TE;",
        )
        .unwrap();
        assert_eq!(m.type_params.len(), 2);
        assert_eq!(m.type_params[0].name, "T");
        assert_eq!(m.type_params[1].bounds.len(), 1);
        assert_eq!(
            m.params[0],
            JType::Class {
                name: "java/util/List".into(),
                args: vec![TypeArg::Extends(JType::TypeVar("T".into()))],
            }
        );
        assert_eq!(m.ret, Some(JType::TypeVar("T".into())));
        assert_eq!(m.throws, vec![JType::TypeVar("E".into())]);
    }

    #[test]
    fn generic_class_and_inner() {
        let c = parse_class_sig(
            "<K:Ljava/lang/Object;V:Ljava/lang/Object;>Ljava/lang/Object;Ljava/util/Map<TK;TV;>;",
        )
        .unwrap();
        assert_eq!(c.type_params.len(), 2);
        assert_eq!(c.super_class, class("java/lang/Object"));
        assert_eq!(c.interfaces.len(), 1);

        assert_eq!(
            parse_field_type("Ljava/util/Map<TK;TV;>.Entry<TK;*>;"),
            Some(JType::Class {
                name: "java/util/Map$Entry".into(),
                args: vec![
                    TypeArg::Exact(JType::TypeVar("K".into())),
                    TypeArg::Unbounded
                ],
            })
        );
    }
}
