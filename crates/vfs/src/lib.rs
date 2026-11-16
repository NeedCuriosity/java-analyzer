//! 虚拟文件系统：给每个路径分配稳定的 [`FileId`]，保存文件内容，并累积"自上次取走以来的变更"。
//!
//! 分析层只认识 `FileId`，从不直接碰磁盘。编辑器里打开的文件内容以编辑器为准（覆盖层），
//! 关闭后回落到磁盘内容。

mod loader;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use rustc_hash::FxHashMap;

pub use crate::loader::{
    is_lombok_config, is_resource_file, is_spring_config, load_java_files, load_lombok_configs,
    load_spring_configs, read_file_lossy, LoadedFile,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(pub u32);

#[derive(Debug, Clone)]
pub enum ChangeKind {
    Create(Arc<str>),
    Modify(Arc<str>),
    Delete,
}

#[derive(Debug, Clone)]
pub struct ChangedFile {
    pub file_id: FileId,
    pub kind: ChangeKind,
}

#[derive(Default)]
struct FileState {
    /// 磁盘上的内容（由加载器或文件监听提供）
    disk: Option<Arc<str>>,
    /// 编辑器覆盖层（`didOpen` / `didChange`）
    overlay: Option<Arc<str>>,
}

impl FileState {
    fn current(&self) -> Option<&Arc<str>> {
        self.overlay.as_ref().or(self.disk.as_ref())
    }
}

#[derive(Default)]
pub struct Vfs {
    paths: Vec<PathBuf>,
    ids: FxHashMap<PathBuf, FileId>,
    files: Vec<FileState>,
    changes: Vec<ChangedFile>,
}

impl Vfs {
    pub fn file_id(&self, path: &Path) -> Option<FileId> {
        self.ids.get(path).copied()
    }

    pub fn file_path(&self, file_id: FileId) -> &Path {
        &self.paths[file_id.0 as usize]
    }

    pub fn file_contents(&self, file_id: FileId) -> Option<&Arc<str>> {
        self.files[file_id.0 as usize].current()
    }

    pub fn exists(&self, file_id: FileId) -> bool {
        self.files[file_id.0 as usize].current().is_some()
    }

    pub fn is_overlaid(&self, file_id: FileId) -> bool {
        self.files[file_id.0 as usize].overlay.is_some()
    }

    /// 所有当前存在的文件。
    pub fn iter(&self) -> impl Iterator<Item = (FileId, &Path)> + '_ {
        self.paths
            .iter()
            .enumerate()
            .filter(|(i, _)| self.files[*i].current().is_some())
            .map(|(i, p)| (FileId(i as u32), p.as_path()))
    }

    fn alloc(&mut self, path: &Path) -> FileId {
        if let Some(id) = self.ids.get(path) {
            return *id;
        }
        let id = FileId(self.paths.len() as u32);
        self.paths.push(path.to_path_buf());
        self.ids.insert(path.to_path_buf(), id);
        self.files.push(FileState::default());
        id
    }

    /// 更新磁盘内容；`None` 表示文件已被删除。
    pub fn set_disk_contents(&mut self, path: &Path, contents: Option<String>) -> FileId {
        let id = self.alloc(path);
        let before = self.files[id.0 as usize].current().cloned();
        self.files[id.0 as usize].disk = contents.map(Arc::from);
        self.record_change(id, before);
        id
    }

    /// 更新编辑器覆盖层；`None` 表示编辑器关闭了该文件。
    pub fn set_overlay(&mut self, path: &Path, contents: Option<String>) -> FileId {
        let id = self.alloc(path);
        let before = self.files[id.0 as usize].current().cloned();
        self.files[id.0 as usize].overlay = contents.map(Arc::from);
        self.record_change(id, before);
        id
    }

    fn record_change(&mut self, id: FileId, before: Option<Arc<str>>) {
        let after = self.files[id.0 as usize].current().cloned();
        let kind = match (before, after) {
            (None, None) => return,
            (None, Some(text)) => ChangeKind::Create(text),
            (Some(_), None) => ChangeKind::Delete,
            (Some(old), Some(new)) => {
                if *old == *new {
                    return;
                }
                ChangeKind::Modify(new)
            }
        };
        self.changes.push(ChangedFile { file_id: id, kind });
    }

    pub fn has_changes(&self) -> bool {
        !self.changes.is_empty()
    }

    /// 取走累积的变更。同一个文件的多次变更会被折叠成最后一次。
    pub fn take_changes(&mut self) -> Vec<ChangedFile> {
        let changes = std::mem::take(&mut self.changes);
        let mut last: FxHashMap<FileId, usize> = FxHashMap::default();
        for (i, change) in changes.iter().enumerate() {
            last.insert(change.file_id, i);
        }
        changes
            .into_iter()
            .enumerate()
            .filter(|(i, c)| last.get(&c.file_id) == Some(i))
            .map(|(_, c)| c)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_wins_and_falls_back_to_disk() {
        let mut vfs = Vfs::default();
        let path = Path::new("/p/A.java");
        let id = vfs.set_disk_contents(path, Some("disk".into()));
        vfs.set_overlay(path, Some("editor".into()));
        assert_eq!(&**vfs.file_contents(id).unwrap(), "editor");
        vfs.set_overlay(path, None);
        assert_eq!(&**vfs.file_contents(id).unwrap(), "disk");

        let changes = vfs.take_changes();
        assert_eq!(changes.len(), 1, "同一文件的变更应折叠");
        assert!(vfs.take_changes().is_empty());
    }

    #[test]
    fn identical_content_is_not_a_change() {
        let mut vfs = Vfs::default();
        let path = Path::new("/p/A.java");
        vfs.set_disk_contents(path, Some("x".into()));
        vfs.take_changes();
        vfs.set_disk_contents(path, Some("x".into()));
        assert!(!vfs.has_changes());
    }
}
