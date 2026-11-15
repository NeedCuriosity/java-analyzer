//! 最近解析结果的旁路缓存。
//!
//! salsa 的 tracked 函数拿不到自己上一次的返回值，而增量重解析需要"旧文本 + 旧语法树"。
//! 这里按文件记住最近一次的解析结果。它只是加速手段：命中与否不改变 `parse` 的返回值。

use std::sync::{Arc, Mutex};

use rustc_hash::FxHashMap;
use syntax::{ast, Parse};
use vfs::FileId;

/// 最多记住这么多个文件；超过就整体清空（正在编辑的文件很快会重新进来）。
const CAPACITY: usize = 64;

type Entry = (Arc<str>, Parse<ast::CompilationUnit>);

#[derive(Default)]
pub struct ParseCache {
    entries: Mutex<FxHashMap<FileId, Entry>>,
}

impl ParseCache {
    pub fn get(&self, file_id: FileId) -> Option<Entry> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&file_id)
            .cloned()
    }

    pub fn put(&self, file_id: FileId, text: Arc<str>, parse: Parse<ast::CompilationUnit>) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() >= CAPACITY && !entries.contains_key(&file_id) {
            entries.clear();
        }
        entries.insert(file_id, (text, parse));
    }
}
