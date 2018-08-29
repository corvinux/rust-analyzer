#[macro_use]
extern crate failure;
extern crate parking_lot;
#[macro_use]
extern crate log;
extern crate once_cell;
extern crate libsyntax2;
extern crate libeditor;
extern crate fst;
extern crate rayon;
extern crate relative_path;

mod symbol_index;
mod module_map;
mod api;

use std::{
    fmt,
    panic,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::SeqCst},
    },
    collections::hash_map::HashMap,
    time::Instant,
};

use relative_path::{RelativePath,RelativePathBuf};
use once_cell::sync::OnceCell;
use rayon::prelude::*;

use libsyntax2::{
    File,
    TextUnit, TextRange, SmolStr,
    ast::{self, AstNode, NameOwner},
    SyntaxKind::*,
};
use libeditor::{LineIndex, FileSymbol, find_node_at_offset};

use self::{
    symbol_index::FileSymbols,
    module_map::{ModuleMap, ChangeKind, Problem},
};
pub use self::symbol_index::Query;
pub use self::api::*;

pub type Result<T> = ::std::result::Result<T, ::failure::Error>;

pub trait FileResolver: Send + Sync + 'static {
    fn file_stem(&self, id: FileId) -> String;
    fn resolve(&self, id: FileId, path: &RelativePath) -> Option<FileId>;
}

#[derive(Debug)]
pub struct WorldState {
    data: Arc<WorldData>
}

pub(crate) struct World {
    needs_reindex: AtomicBool,
    file_resolver: Arc<FileResolver>,
    data: Arc<WorldData>,
}

impl fmt::Debug for World {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        (&*self.data).fmt(f)
    }
}

impl Clone for World {
    fn clone(&self) -> World {
        World {
            needs_reindex: AtomicBool::new(self.needs_reindex.load(SeqCst)),
            file_resolver: Arc::clone(&self.file_resolver),
            data: Arc::clone(&self.data),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(pub u32);

impl WorldState {
    pub fn new() -> WorldState {
        WorldState {
            data: Arc::new(WorldData::default()),
        }
    }

    pub fn analysis(
        &self,
        file_resolver: impl FileResolver,
    ) -> Analysis {
        let imp = World {
            needs_reindex: AtomicBool::new(false),
            file_resolver: Arc::new(file_resolver),
            data: self.data.clone()
        };
        Analysis { imp }
    }

    pub fn change_file(&mut self, file_id: FileId, text: Option<String>) {
        self.change_files(::std::iter::once((file_id, text)));
    }

    pub fn change_files(&mut self, changes: impl Iterator<Item=(FileId, Option<String>)>) {
        let data = self.data_mut();
        for (file_id, text) in changes {
            let change_kind = if data.file_map.remove(&file_id).is_some() {
                if text.is_some() {
                    ChangeKind::Update
                } else {
                    ChangeKind::Delete
                }
            } else {
                ChangeKind::Insert
            };
            data.module_map.update_file(file_id, change_kind);
            data.file_map.remove(&file_id);
            if let Some(text) = text {
                let file_data = FileData::new(text);
                data.file_map.insert(file_id, Arc::new(file_data));
            } else {
                data.file_map.remove(&file_id);
            }
        }
    }

    fn data_mut(&mut self) -> &mut WorldData {
        if Arc::get_mut(&mut self.data).is_none() {
            self.data = Arc::new(WorldData {
                file_map: self.data.file_map.clone(),
                module_map: self.data.module_map.clone(),
            });
        }
        Arc::get_mut(&mut self.data).unwrap()
    }
}

#[derive(Debug)]
pub struct QuickFix {
    pub fs_ops: Vec<FsOp>,
}

#[derive(Debug)]
pub enum FsOp {
    CreateFile {
        anchor: FileId,
        path: RelativePathBuf,
    },
    MoveFile {
        file: FileId,
        path: RelativePathBuf,
    }
}

impl World {
    pub fn file_syntax(&self, file_id: FileId) -> Result<File> {
        let data = self.file_data(file_id)?;
        Ok(data.syntax().clone())
    }

    pub fn file_line_index(&self, id: FileId) -> Result<LineIndex> {
        let data = self.file_data(id)?;
        let index = data.lines
            .get_or_init(|| LineIndex::new(&data.text));
        Ok(index.clone())
    }

    pub fn world_symbols(&self, mut query: Query) -> Vec<(FileId, FileSymbol)> {
        self.reindex();
        self.data.file_map.iter()
            .flat_map(move |(id, data)| {
                let symbols = data.symbols();
                query.process(symbols).into_iter().map(move |s| (*id, s))
            })
            .collect()
    }

    pub fn parent_module(&self, id: FileId) -> Vec<(FileId, FileSymbol)> {
        let module_map = &self.data.module_map;
        let id = module_map.file2module(id);
        module_map
            .parent_modules(
                id,
                &*self.file_resolver,
                &|file_id| self.file_syntax(file_id).unwrap(),
            )
            .into_iter()
            .map(|(id, name, node)| {
                let id = module_map.module2file(id);
                let sym = FileSymbol {
                    name,
                    node_range: node.range(),
                    kind: MODULE,
                };
                (id, sym)
            })
            .collect()
    }

    pub fn approximately_resolve_symbol(
        &self,
        id: FileId,
        offset: TextUnit,
    ) -> Result<Vec<(FileId, FileSymbol)>> {
        let file = self.file_syntax(id)?;
        let syntax = file.syntax();
        if let Some(name_ref) = find_node_at_offset::<ast::NameRef>(syntax, offset) {
            return Ok(self.index_resolve(name_ref));
        }
        if let Some(name) = find_node_at_offset::<ast::Name>(syntax, offset) {
            if let Some(module) = name.syntax().parent().and_then(ast::Module::cast) {
                if module.has_semi() {
                    let file_ids = self.resolve_module(id, module);

                    let res = file_ids.into_iter().map(|id| {
                        let name = module.name()
                            .map(|n| n.text())
                            .unwrap_or_else(|| SmolStr::new(""));
                        let symbol = FileSymbol {
                            name,
                            node_range: TextRange::offset_len(0.into(), 0.into()),
                            kind: MODULE,
                        };
                        (id, symbol)
                    }).collect();

                    return Ok(res);
                }
            }
        }
        Ok(vec![])
    }

    pub fn diagnostics(&self, file_id: FileId) -> Vec<Diagnostic> {
        let syntax = self.file_syntax(file_id).unwrap();
        let mut res = libeditor::diagnostics(&syntax)
            .into_iter()
            .map(|d| Diagnostic { range: d.range, message: d.msg, fix: None })
            .collect::<Vec<_>>();

        self.data.module_map.problems(
            file_id,
            &*self.file_resolver,
            &|file_id| self.file_syntax(file_id).unwrap(),
            |name_node, problem| {
                let diag = match problem {
                    Problem::UnresolvedModule { candidate } => {
                        let create_file = FileSystemEdit::CreateFile {
                            anchor: file_id,
                            path: candidate.clone(),
                        };
                        let fix = SourceChange {
                            label: "create module".to_string(),
                            source_file_edits: Vec::new(),
                            file_system_edits: vec![create_file],
                            cursor_position: None,
                        };
                        Diagnostic {
                            range: name_node.syntax().range(),
                            message: "unresolved module".to_string(),
                            fix: Some(fix),
                        }
                    }
                    Problem::NotDirOwner { move_to, candidate } => {
                        let move_file = FileSystemEdit::MoveFile { file: file_id, path: move_to.clone() };
                        let create_file = FileSystemEdit::CreateFile { anchor: file_id, path: move_to.join(candidate) };
                        let fix = SourceChange {
                            label: "move file and create module".to_string(),
                            source_file_edits: Vec::new(),
                            file_system_edits: vec![move_file, create_file],
                            cursor_position: None,
                        };
                        Diagnostic {
                            range: name_node.syntax().range(),
                            message: "can't declare module at this location".to_string(),
                            fix: Some(fix),
                        }
                    }
                };
                res.push(diag)
            }
        );
        res
    }

    pub fn assists(&self, file_id: FileId, offset: TextUnit) -> Vec<SourceChange> {
        let file = self.file_syntax(file_id).unwrap();
        let actions = vec![
            ("flip comma", libeditor::flip_comma(&file, offset).map(|f| f())),
            ("add `#[derive]`", libeditor::add_derive(&file, offset).map(|f| f())),
            ("add impl", libeditor::add_impl(&file, offset).map(|f| f())),
        ];
        let mut res = Vec::new();
        for (name, local_edit) in actions {
            if let Some(local_edit) = local_edit {
                res.push(SourceChange::from_local_edit(
                    file_id, name, local_edit
                ))
            }
        }
        res
    }

    fn index_resolve(&self, name_ref: ast::NameRef) -> Vec<(FileId, FileSymbol)> {
        let name = name_ref.text();
        let mut query = Query::new(name.to_string());
        query.exact();
        query.limit(4);
        self.world_symbols(query)
    }

    fn resolve_module(&self, id: FileId, module: ast::Module) -> Vec<FileId> {
        let name = match module.name() {
            Some(name) => name.text(),
            None => return Vec::new(),
        };
        let module_map = &self.data.module_map;
        let id = module_map.file2module(id);
        module_map
            .child_module_by_name(
                id, name.as_str(),
                &*self.file_resolver,
                &|file_id| self.file_syntax(file_id).unwrap(),
            )
            .into_iter()
            .map(|id| module_map.module2file(id))
            .collect()
    }

    fn reindex(&self) {
        if self.needs_reindex.compare_and_swap(false, true, SeqCst) {
            let now = Instant::now();
            let data = &*self.data;
            data.file_map
                .par_iter()
                .for_each(|(_, data)| drop(data.symbols()));
            info!("parallel indexing took {:?}", now.elapsed());
        }
    }

    fn file_data(&self, file_id: FileId) -> Result<Arc<FileData>> {
        match self.data.file_map.get(&file_id) {
            Some(data) => Ok(data.clone()),
            None => bail!("unknown file: {:?}", file_id),
        }
    }
}

#[derive(Default, Debug)]
struct WorldData {
    file_map: HashMap<FileId, Arc<FileData>>,
    module_map: ModuleMap,
}

#[derive(Debug)]
struct FileData {
    text: String,
    symbols: OnceCell<FileSymbols>,
    syntax: OnceCell<File>,
    lines: OnceCell<LineIndex>,
}

impl FileData {
    fn new(text: String) -> FileData {
        FileData {
            text,
            symbols: OnceCell::new(),
            syntax: OnceCell::new(),
            lines: OnceCell::new(),
        }
    }

    fn syntax(&self) -> &File {
        let text = &self.text;
        let syntax = &self.syntax;
        match panic::catch_unwind(panic::AssertUnwindSafe(|| syntax.get_or_init(|| File::parse(text)))) {
            Ok(file) => file,
            Err(err) => {
                error!("Parser paniced on:\n------\n{}\n------\n", &self.text);
                panic::resume_unwind(err)
            }
        }
    }

    fn syntax_transient(&self) -> File {
        self.syntax.get().map(|s| s.clone())
            .unwrap_or_else(|| File::parse(&self.text))
    }

    fn symbols(&self) -> &FileSymbols {
        let syntax = self.syntax_transient();
        self.symbols
            .get_or_init(|| FileSymbols::new(&syntax))
    }
}
