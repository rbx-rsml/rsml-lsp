#![feature(generic_const_exprs)]

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::ops::{Deref, DerefMut};
use std::path::{PathBuf};
use std::sync::Arc;

use ropey::Rope;
use serde_json::Value;
use tokio::fs;
use tokio::sync::{Mutex, MutexGuard};
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::notification::{ShowMessage};
use tower_lsp::lsp_types::{CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, CodeActionProviderCapability, CodeActionResponse, DidChangeWatchedFilesRegistrationOptions, FileChangeType, FileSystemWatcher, GlobPattern, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents, HoverParams, HoverProviderCapability, InitializeParams, InitializeResult, LocationLink, MarkedString, MessageType, OneOf, Position, Range, Registration, RelativePattern, ServerCapabilities, ShowMessageParams, TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Url, WatchKind, WorkspaceEdit};
use tower_lsp::{Client, LanguageServer, LspService, Server};

mod lexer;
use lexer::Lexer;

mod parser;
use parser::Parser;

mod typechecker;
use typechecker::Typechecker;

pub mod range_from_span;

use crate::lexer::SpannedToken;
use crate::range_from_span::RangeFromSpan;
use crate::typechecker::{DefinitionKind, Definitions};

mod guarded_unwrap;  

mod list;

mod luaurc;
use luaurc::Luaurc;

pub mod normalize_path;

mod string_clip {
    pub trait StringClip {
        fn clip<'a>(&'a self, start: usize, end: usize) -> &'a str;
    }
    
    impl StringClip for str {
        fn clip<'a>(&'a self, start: usize, end: usize) -> &'a str {
            &self[start..self.len() - end]
        }
    }
}

struct Workspaces(HashMap<PathBuf, Arc<Mutex<HashMap<PathBuf, Arc<Mutex<Document>>>>>>);

impl Deref for Workspaces {
    type Target = HashMap<PathBuf, Arc<Mutex<HashMap<PathBuf, Arc<Mutex<Document>>>>>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Workspaces {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Workspaces {
    fn new() -> Self {
        Self(HashMap::new())
    }

    fn get_for_path(&self, path: &PathBuf) -> Option<Arc<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<Document>>>>>> {
        match self.iter().find(|(x, _)| path.starts_with(x)) {
            Some(workspace) => Some(workspace.1.clone()),
            None => None
        }
    }
}

struct Backend {
    client: Client,
    luaurcs: Arc<Mutex<HashMap<PathBuf, Luaurc>>>,
    workspaces: Arc<Mutex<Workspaces>>,
    has_root: Mutex<bool>
}


struct Document {
    source: String,
    dependencies: HashSet<PathBuf>,
    definitions: Definitions
}

enum Status<T> {
    Some(T),
    Unknown,
    None,
}

impl Document {
    fn new(source: String) -> Self {
        Self {
            source,
            dependencies: HashSet::new(),
            definitions: Definitions::new()
        }
    }
}


#[macro_export]
macro_rules! collection {
    // map-like
    ($($k:expr => $v:expr),* $(,)?) => {{
        use std::iter::{Iterator, IntoIterator};
        Iterator::collect(IntoIterator::into_iter([$(($k, $v),)*]))
    }};
    // set-like
    ($($v:expr),* $(,)?) => {{
        use std::iter::{Iterator, IntoIterator};
        Iterator::collect(IntoIterator::into_iter([$($v,)*]))
    }};
}

#[macro_export]
macro_rules! lazy_collection {
    // map-like
    ($($k:expr => $v:expr),* $(,)?) => {{
        use std::iter::{Iterator, IntoIterator};
        LazyLock::new(|| Iterator::collect(IntoIterator::into_iter([$(($k, $v),)*])))
    }};
    // set-like
    ($($v:expr),* $(,)?) => {{
        use std::iter::{Iterator, IntoIterator};
        LazyLock::new(|| Iterator::collect(IntoIterator::into_iter([$($v,)*])))
    }};
}

async fn resolve_luaurc(path: PathBuf, luaurcs: &mut HashMap<PathBuf, Luaurc>) {
    let contents = match fs::read_to_string(path.join("./luaurc")).await {
        Ok(contents) => Ok(contents),
        Err (_) => fs::read_to_string(path.join("./.luaurc")).await
    };

    if let Ok(contents) = contents {
        luaurcs.insert(path, Luaurc::new(&contents));
    }
}

trait PositionToByteOffset {
    fn byte_offset(&self, text: &str) -> usize;
}

impl PositionToByteOffset for Position {
    fn byte_offset(&self, text: &str) -> usize {
        let mut line_start = 0usize;
        let mut current_line = 0usize;

        for (idx, c) in text.char_indices() {
            if current_line == self.line as usize {
                line_start = idx;
                break;
            }
            if c == '\n' {
                current_line += 1;
            }
        }

        // If the position line is beyond the text, use end of text
        if current_line < self.line as usize {
            return text.len();
        }

        // Now calculate the byte offset of the UTF-16 character index
        let mut offset_bytes = line_start;
        let mut utf16_count = 0;
        for (idx, c) in text[line_start..].char_indices() {
            if utf16_count >= self.character as usize {
                break;
            }
            utf16_count += c.len_utf16();
            offset_bytes = line_start + idx + c.len_utf8();
        }

        offset_bytes
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> tower_lsp::jsonrpc::Result<InitializeResult> {
        if let Some(folders) = params.workspace_folders {
            let mut luaurcs = self.luaurcs.lock().await;

            for folder in folders {
                if let Ok(path) = folder.uri.to_file_path() {
                    self.workspaces.lock().await.insert(path.clone(), Arc::new(Mutex::new(HashMap::new())));
                    resolve_luaurc(path, &mut luaurcs).await;
                }
            }
            
        } else if let Some(root_uri) = params.root_uri {
            let mut luaurcs = self.luaurcs.lock().await;
            
            if let Ok(path) = root_uri.to_file_path() {
                self.workspaces.lock().await.insert(path.clone(), Arc::new(Mutex::new(HashMap::new())));
                resolve_luaurc(path, &mut luaurcs).await;
            }

        } else {
            *self.has_root.lock().await = false;
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),

                ..ServerCapabilities::default()
            },
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _: tower_lsp::lsp_types::InitializedParams) {
        if !*self.has_root.lock().await {
            self.notify(
                MessageType::WARNING,
                format!("Could not resolve root for your workspace(s). `.luaurc`'s aliases may not work properly in derives.")
            ).await;
        }

        if let Ok(Some(workspace_folders)) = self.client.workspace_folders().await {
            let registration = Registration {
                id: "config-watcher".to_string(),
                method: "workspace/didChangeWatchedFiles".to_string(),
                register_options: Some(serde_json::to_value(
                    DidChangeWatchedFilesRegistrationOptions {
                        watchers: workspace_folders.iter()
                            .map(|x| FileSystemWatcher {
                                glob_pattern: GlobPattern::Relative(RelativePattern {
                                    base_uri: OneOf::Right(x.uri.clone()),
                                    pattern: String::from("{luaurc,*.luaurc}"),
                                }),
                                kind: Some(WatchKind::Create | WatchKind::Change | WatchKind::Delete),
                            })
                            .collect::<Vec<_>>(),
                    }
                ).unwrap()),
            };

            self.client.register_capability(vec![registration]).await.unwrap();
        }
    }

    async fn shutdown(&self) -> tower_lsp::jsonrpc::Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: tower_lsp::lsp_types::DidOpenTextDocumentParams) {
        let mut workspaces = self.workspaces.lock().await;

        let (current_path, document) =
            self.parse_and_log(&params.text_document.text, params.text_document.uri, Status::Unknown, &mut workspaces).await;

        self.commit_document(current_path, document, &mut workspaces).await;
    }

    async fn did_change(&self, params: tower_lsp::lsp_types::DidChangeTextDocumentParams) {
        if let Some(change) = params.content_changes.into_iter().next() {
            let mut workspaces = self.workspaces.lock().await;

            let (current_path, document) =
                self.parse_and_log(&change.text, params.text_document.uri, Status::Unknown, &mut workspaces).await;

            self.commit_document(current_path, document, &mut workspaces).await;
        }
    }

    async fn did_change_watched_files(&self, params: tower_lsp::lsp_types::DidChangeWatchedFilesParams) {
        for change in params.changes {
            let path = guarded_unwrap!(change.uri.to_file_path(), return);

            if !path.is_file() { return };

            let prefix = path.file_prefix();

            if !(
                prefix == Some(OsStr::new("luaurc")) || 
                prefix == Some(OsStr::new(".luaurc")) || 
                path.extension() == Some(OsStr::new("luaurc"))
            ) { return }

            let base_path = guarded_unwrap!(path.join("../").canonicalize(), return);

            let mut luaurcs = self.luaurcs.lock().await;

            match change.typ {
                FileChangeType::CHANGED => {
                    luaurcs.remove(&base_path);

                    let mut luaurc = if let Ok(contents) = fs::read_to_string(path).await {
                        Luaurc::new(&contents)
                    } else {
                        Luaurc { aliases: HashMap::new() }
                    };

                    let mut workspaces = self.workspaces.lock().await;
                    let workspace_mutex = guarded_unwrap!(
                        workspaces.get(&base_path).cloned(), return
                    );
                    let mut workspace = workspace_mutex.lock().await;

                    let mut new_documents = HashMap::new();

                    for (document_path, document) in workspace.drain() {
                        let document = document.lock().await;

                        let uri = guarded_unwrap!(Url::from_file_path(&document_path), continue);

                        let (_, document) = self.parse_and_log(
                            &document.source, uri, Status::Some(&mut luaurc),
                            &mut workspaces
                        ).await;

                        new_documents.insert(document_path.clone(), Arc::new(Mutex::new(document)));
                    }

                    workspaces.insert(base_path.clone(), Arc::new(Mutex::new(new_documents)));
                    luaurcs.insert(base_path, luaurc);
                },

                FileChangeType::CREATED => {
                    let luaurc = if let Ok(contents) = fs::read_to_string(path).await {
                        Luaurc::new(&contents)
                    } else {
                        Luaurc { aliases: HashMap::new() }
                    };

                    luaurcs.insert(base_path, luaurc);
                }

                FileChangeType::DELETED => {
                    guarded_unwrap!(luaurcs.remove(&base_path), return);

                    let mut workspaces = self.workspaces.lock().await;
                    let workspace_mutex = guarded_unwrap!(
                        workspaces.get(&base_path).cloned(), return
                    );
                    let mut workspace = workspace_mutex.lock().await;

                    let mut new_documents = HashMap::new();

                    for (document_path, document) in workspace.drain() {
                        let document = document.lock().await;

                        let uri = guarded_unwrap!(Url::from_file_path(&document_path), continue);

                        let (_, document) = self.parse_and_log(&document.source, uri, Status::None, &mut workspaces).await;

                        new_documents.insert(document_path.clone(), Arc::new(Mutex::new(document)));
                    }

                    workspaces.insert(base_path, Arc::new(Mutex::new(new_documents)));
                },

                _ => ()
            };
        }
    }

    async fn goto_definition(&self, params: GotoDefinitionParams) -> LspResult<Option<GotoDefinitionResponse>> {
        let position = params.text_document_position_params.position;
        let uri = params.text_document_position_params.text_document.uri;

        let current_path = guarded_unwrap!(uri.to_file_path(), return Ok(None));

        let workspaces = self.workspaces.lock().await;
        let workspace_mutex = guarded_unwrap!(
            workspaces.get_for_path(&current_path), return Ok(None)
        );
        let workspace = workspace_mutex.lock().await;

        let document = workspace.get(&current_path);

        if let Some(document) = document {
            let document = document.lock().await;

            let byte_offset = position.byte_offset(&document.source);

            if let Some((
                span, kind
            )) = document.definitions.get_key_value(&byte_offset) {
                let rope = Rope::from_str(&document.source);

                match kind {
                    DefinitionKind::Derive { path } => {
                        if let Ok(target_uri) = Url::from_file_path(path) {
                            return Ok(Some(GotoDefinitionResponse::Link(vec![LocationLink {
                                origin_selection_range: Some(Range::from_span(&rope, (*span.start(), *span.end()))),
                                target_uri,
                                target_range: Range {
                                    start: Position { line: 0, character: 0 },
                                    end: Position { line: 0, character: 0 },
                                },
                                target_selection_range: Range {
                                    start: Position { line: 0, character: 0 },
                                    end: Position { line: 0, character: 0 },
                                },
                            }])));
                        }
                    },

                    DefinitionKind::Selector { .. } => ()
                }
            }
        };

        return Ok(None)
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>, tower_lsp::jsonrpc::Error> {
        let position = params.text_document_position_params.position;
        let uri = params.text_document_position_params.text_document.uri;

        let current_path = guarded_unwrap!(uri.to_file_path(), return Ok(None));

        let workspaces = self.workspaces.lock().await;
        let workspace_mutex = guarded_unwrap!(
            workspaces.get_for_path(&current_path), return Ok(None)
        );
        let workspace = workspace_mutex.lock().await;

        let document = workspace.get(&current_path);

        if let Some(document) = document {
            let document = document.lock().await;

            let byte_offset = position.byte_offset(&document.source);

            if let Some((
                span, kind
            )) = document.definitions.get_key_value(&byte_offset) {
                let rope = Rope::from_str(&document.source);

                let contents = match kind {
                    DefinitionKind::Derive { path } => {
                        HoverContents::Scalar(MarkedString::from_markdown(
                            format!("```luau\n{:#?}\n```", path)
                        ))
                    }

                    DefinitionKind::Selector { hint, .. } => {
                        HoverContents::Scalar(MarkedString::from_markdown(
                            format!("```luau\n{}\n```", hint.to_string())
                        ))
                    },
                };

                return Ok(Some(Hover {
                    contents,
                    range: Some(Range::from_span(&rope, (*span.start(), *span.end()))),
                }))
            }
        };

        Ok(None)
    }

    async fn code_action(&self, params: CodeActionParams) -> LspResult<Option<CodeActionResponse>> {
        let diagnostics = &params.context.diagnostics;

        let mut code_actions = vec![];

        let uri = params.text_document.uri;

        for diagnostic in diagnostics {
            let data =
                if let Some(Value::Object(data)) = &diagnostic.data { data }
                else { continue };

            let closest = 
                if let Some(Value::String(data)) = data.get("closest") { data }
                else { continue };

            let range_start = 
                if let Some(Value::Object(range_start)) = data.get("range_start") { range_start }
                else { continue };

            let start_line = 
                if let Some(Value::Number(start_line)) = range_start.get("line") {
                    if let Some(start_line) = start_line.as_u64() { start_line as u32 }
                    else { continue }
                } else { continue };
            
            let start_char = 
                if let Some(Value::Number(start_char)) = range_start.get("char") {
                    if let Some(start_char) = start_char.as_u64() { start_char as u32 }
                    else { continue }
                } else { continue };

            let range_end = 
                if let Some(Value::Object(range_end)) = data.get("range_end") { range_end }
                else { continue };

            let end_line = 
                if let Some(Value::Number(end_line)) = range_end.get("line") {
                    if let Some(end_line) = end_line.as_u64() { end_line as u32 }
                    else { continue }
                } else { continue };

            let end_char = 
                if let Some(Value::Number(end_char)) = range_end.get("char") {
                    if let Some(end_char) = end_char.as_u64() { end_char as u32 }
                    else { continue }
                } else { continue };

            let edit = WorkspaceEdit {
                changes: Some(collection! {
                    uri.clone() => vec![TextEdit {
                        range: Range {
                            start: Position::new(start_line, start_char),
                            end: Position::new(end_line, end_char),
                        },
                        new_text: closest.into(),
                    }]
                }),
                ..Default::default()
            };

            code_actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: format!("Replace with {}", closest).into(),
                kind: Some(CodeActionKind::QUICKFIX),
                edit: Some(edit),
                ..Default::default()
            }));
        };

        Ok(Some(code_actions))
    }
}

impl Backend {
    fn workspace_path_for_path(&self, path: &PathBuf, workspaces: &mut Workspaces) -> Option<PathBuf> {
        workspaces
            .iter()
            .find(|(x, _)| path.starts_with(x))
            .map(|(x, _)| x)
            .cloned()
    }

    async fn notify(&self, ty: MessageType, msg: String) 
    {
        self.client.send_notification::<ShowMessage>(
            ShowMessageParams {
                typ: ty,
                message: msg,
            },
        ).await;
    }

    async fn commit_document(&self, current_path: Option<PathBuf>, document: Document, workspaces: &mut Workspaces) {
        if let Some(current_path) = current_path {
            if let Some(documents) = workspaces.get_for_path(&current_path) {
                documents.lock().await.insert(current_path, Arc::new(Mutex::new(document)));
            }
        }
    }

    async fn parse_and_log(
        &self,
        source_code: &str,
        uri: Url,
        luaurc: Status<&mut Luaurc>,
        workspaces: &mut Workspaces
    ) -> (Option<PathBuf>, Document) {
        let mut document = Document::new(source_code.to_string());

        let (diagnostics, current_path) = if source_code.len() == 0 {
            (vec![], None)

        } else {
            let parsed = Parser::new(Lexer::new(source_code));

            let uri_file_path = uri.to_file_path();

            let (typechecked, current_path) = match uri_file_path {
                Ok(current_path) => {
                    let typechecked = match luaurc {
                        Status::Some(luaurc) =>
                            Typechecker::new(parsed, &current_path, workspaces, &mut document, Some(luaurc)),

                        Status::Unknown => {
                            let luaurcs = self.luaurcs.lock().await;

                            let luaurc =
                                if let Some(workspace_path) = &self.workspace_path_for_path(&current_path, workspaces) {
                                    luaurcs.get(workspace_path)
                                } else { None };

                            Typechecker::new(parsed, &current_path, workspaces, &mut document, luaurc)
                        },

                        Status::None =>
                            Typechecker::new(parsed, &current_path, workspaces, &mut document, None)
                    };

                    (typechecked, Some(current_path))
                },

                Err(_) => {
                    self.notify(
                        MessageType::WARNING,
                        String::from("Could not get the path for the current files. You may experience issues with derives being resolved.")
                    ).await;

                    (Typechecker::new(parsed, &PathBuf::from("/"), workspaces, &mut document, None), None)
                }
            };

            (typechecked.parsed.ast_errors.0, current_path)
        };

        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;

        (current_path, document)
    }
}

async fn watch() {
    let (stdin, stdout) = (tokio::io::stdin(), tokio::io::stdout());

    let (service, socket) = LspService::new(|client| Backend {
        client,
        workspaces: Arc::new(Mutex::new(Workspaces::new())),
        luaurcs: Arc::new(Mutex::new(HashMap::new())),
        has_root: Mutex::new(true)
    });

    Server::new(stdin, stdout, socket).serve(service).await;
}

async fn test() {
    let contents = fs::read_to_string("./test/test.rsml").await.unwrap();

    let lexed = Lexer::new(&contents);
    println!("{:#?}", lexed.collect::<Vec<SpannedToken>>());

    let parsed: Parser<'_> = Parser::new(Lexer::new(&contents));
    //println!("{:#?} {:#?}", parsed.ast, parsed.ast_errors);

    let typechecked = Typechecker::new(
        parsed, &PathBuf::from("/"), &mut Workspaces::new(), &mut Document::new(contents.clone()), None
    );

    println!("{:#?} {:#?}", typechecked.parsed.ast, typechecked.parsed.ast_errors);
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.contains(&"--test".to_string()) {
        test().await;
    } else {
        watch().await;
    }
}