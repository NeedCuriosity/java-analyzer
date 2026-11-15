//! 进程级字符串驻留器。

use std::sync::RwLock;

use rustc_hash::FxHashMap;
use smol_str::SmolStr;

#[derive(Default)]
struct Inner {
    ids: FxHashMap<SmolStr, u32>,
    names: Vec<SmolStr>,
}

/// 只增不减：id 一旦分配就永远指向同一个字符串。
#[derive(Default)]
pub struct Interner {
    inner: RwLock<Inner>,
}

impl Interner {
    pub fn intern(&self, text: &str) -> u32 {
        if let Some(id) = self
            .inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .ids
            .get(text)
        {
            return *id;
        }
        let mut inner = self.inner.write().unwrap_or_else(|e| e.into_inner());
        if let Some(id) = inner.ids.get(text) {
            return *id;
        }
        let id = inner.names.len() as u32;
        let name = SmolStr::new(text);
        inner.names.push(name.clone());
        inner.ids.insert(name, id);
        id
    }

    /// 只查不建。
    pub fn get(&self, text: &str) -> Option<u32> {
        self.inner
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .ids
            .get(text)
            .copied()
    }

    pub fn lookup(&self, id: u32) -> SmolStr {
        self.inner.read().unwrap_or_else(|e| e.into_inner()).names[id as usize].clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_stable() {
        let interner = Interner::default();
        let a = interner.intern("java/lang/String");
        let b = interner.intern("java/lang/Object");
        assert_ne!(a, b);
        assert_eq!(interner.intern("java/lang/String"), a);
        assert_eq!(interner.lookup(b), "java/lang/Object");
        assert_eq!(interner.get("missing"), None);
    }
}
