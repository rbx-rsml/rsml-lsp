#![feature(iter_intersperse)]

use std::collections::{HashSet, VecDeque};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use tokio::fs;
use tokio::sync::{Mutex, MutexGuard};
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::notification::ShowMessage;
use tower_lsp::lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, CodeActionResponse, CompletionItem, CompletionItemKind,
    CompletionOptions, CompletionParams, CompletionResponse, DeleteFilesParams,
    DidChangeWatchedFilesRegistrationOptions, DidChangeWorkspaceFoldersParams, FileChangeType,
    FileOperationFilter, FileOperationPattern, FileOperationRegistrationOptions, FileSystemWatcher,
    GlobPattern, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents, HoverParams,
    HoverProviderCapability, InitializeParams, InitializeResult, LocationLink, MarkedString,
    MessageType, NumberOrString, OneOf, Position, Range, Registration, RelativePattern,
    ServerCapabilities, ShowMessageParams, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextEdit, Url, WatchKind, WorkspaceEdit, WorkspaceFileOperationsServerCapabilities,
    WorkspaceServerCapabilities,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

use ropey::Rope;

use rbx_rsml::datatype::Datatype;
use rbx_rsml::lexer::{RsmlLexer, SpannedToken};
use rbx_rsml::parser::RsmlParser;
use rbx_rsml::range_from_span::RangeFromSpan;
use rbx_rsml::typechecker::{
    CyclicKind, DefinitionKind, ReportTypeError, ResolvedTypeKey, ResolvedTypes, TypeError,
    TypecheckedRsml, Typechecker,
    luaurc::Luaurc,
};
use rbx_types::Variant;

pub mod autocomplete;
pub mod workspaces;
use workspaces::{Document, Documents, Workspace, Workspaces};

fn format_token_hint(name: &str, is_static: bool, resolved_types: &ResolvedTypes) -> String {
    let sigil = if is_static { "$!" } else { "$" };
    let type_name = resolved_types
        .get(&ResolvedTypeKey::Token {
            name: name.to_string(),
            is_static,
        })
        .map(|dt| match dt {
            Datatype::IncompleteEnumShorthand(_) => format!("Enum.{}", name),
            Datatype::Variant(Variant::EnumItem(item)) => format!("Enum.{}", item.ty),
            other => other.type_name(),
        })
        .unwrap_or_else(|| "unknown".to_string());
    format!("{}{}: {}", sigil, name, type_name)
}

fn convert_range(range: rbx_rsml::types::Range) -> Range {
    Range {
        start: Position {
            line: range.start.line,
            character: range.start.character,
        },
        end: Position {
            line: range.end.line,
            character: range.end.character,
        },
    }
}

fn convert_diagnostic(diag: rbx_rsml::types::Diagnostic) -> tower_lsp::lsp_types::Diagnostic {
    tower_lsp::lsp_types::Diagnostic {
        range: convert_range(diag.range),
        severity: Some(match diag.severity {
            rbx_rsml::types::Severity::Error => tower_lsp::lsp_types::DiagnosticSeverity::ERROR,
            rbx_rsml::types::Severity::Warning => tower_lsp::lsp_types::DiagnosticSeverity::WARNING,
        }),
        code: Some(NumberOrString::String(diag.code)),
        code_description: None,
        source: Some(String::from("RSML LSP")),
        message: diag.message,
        related_information: None,
        tags: None,
        data: diag.data,
    }
}

struct Backend {
    client: Client,
    workspaces: Arc<Mutex<Workspaces>>,
    documents: Arc<Mutex<Documents>>,
}

enum Status<T> {
    Some(T),
    Unknown,
    None,
}

impl<T> Status<T> {
    pub fn as_deref_mut(&mut self) -> Status<&mut T::Target>
    where
        T: std::ops::DerefMut,
    {
        match self {
            Status::Some(value) => Status::Some(value.deref_mut()),
            Status::Unknown => Status::Unknown,
            Status::None => Status::None,
        }
    }
}

async fn resolve_luaurc(path: &Path) -> Option<Luaurc> {
    let contents = match fs::read_to_string(path.join("./luaurc")).await {
        Ok(contents) => Ok(contents),
        Err(_) => fs::read_to_string(path.join("./.luaurc")).await,
    };

    if let Ok(contents) = contents {
        Some(Luaurc::new(&contents))
    } else {
        None
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

        if current_line < self.line as usize {
            return text.len();
        }

        // We calculate the byte offset of the character index.
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
    async fn initialize(
        &self,
        params: InitializeParams,
    ) -> tower_lsp::jsonrpc::Result<InitializeResult> {
        let mut workspaces = self.workspaces.lock().await;

        if let Some(folders) = params.workspace_folders {
            for folder in folders {
                if let Ok(workspace_path) = folder.uri.to_file_path() {
                    let workspace = Workspace::new(resolve_luaurc(&workspace_path).await);
                    workspaces.insert(workspace_path.clone(), Arc::new(Mutex::new(workspace)));
                }
            }
        } else if let Some(root_uri) = params.root_uri {
            if let Ok(workspace_path) = root_uri.to_file_path() {
                let workspace = Workspace::new(resolve_luaurc(&workspace_path).await);
                workspaces.insert(workspace_path.clone(), Arc::new(Mutex::new(workspace)));
            }
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),

                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),

                definition_provider: Some(OneOf::Left(true)),

                hover_provider: Some(HoverProviderCapability::Simple(true)),

                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![
                        ":".to_string(),
                        ".".to_string(),
                        "=".to_string(),
                        "$".to_string(),
                    ]),
                    ..CompletionOptions::default()
                }),

                workspace: Some(WorkspaceServerCapabilities {
                    file_operations: Some(WorkspaceFileOperationsServerCapabilities {
                        did_delete: Some(FileOperationRegistrationOptions {
                            filters: vec![FileOperationFilter {
                                scheme: Some("file".to_string()),
                                pattern: FileOperationPattern {
                                    glob: "**/*.rsml".to_string(),
                                    matches: None,
                                    options: None,
                                },
                            }],
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),

                ..ServerCapabilities::default()
            },
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _: tower_lsp::lsp_types::InitializedParams) {
        if let Ok(Some(workspace_folders)) = self.client.workspace_folders().await {
            let workspaces = self.workspaces.lock().await;
            let mut documents = self.documents.lock().await;

            for workspace_path in workspaces.keys() {
                self.populate_workspace(workspace_path.clone(), &workspaces, &mut documents)
                    .await;
            }

            let registration = Registration {
                id: "config-watcher".to_string(),
                method: "workspace/didChangeWatchedFiles".to_string(),
                register_options: Some(
                    serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                        watchers: workspace_folders
                            .iter()
                            .map(|x| FileSystemWatcher {
                                glob_pattern: GlobPattern::Relative(RelativePattern {
                                    base_uri: OneOf::Right(x.uri.clone()),
                                    pattern: String::from("{luaurc,*.luaurc}"),
                                }),
                                kind: Some(
                                    WatchKind::Create | WatchKind::Change | WatchKind::Delete,
                                ),
                            })
                            .collect::<Vec<_>>(),
                    })
                    .unwrap(),
                ),
            };

            self.client
                .register_capability(vec![registration])
                .await
                .unwrap();
        } else {
            self.notify(
                MessageType::WARNING,
                format!("Could not resolve root for your workspace(s). `.luaurc`'s aliases may not work properly in derives.")
            ).await;
        }
    }

    async fn shutdown(&self) -> tower_lsp::jsonrpc::Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: tower_lsp::lsp_types::DidOpenTextDocumentParams) {
        let mut documents = self.documents.lock().await;

        let (current_path, document) = self
            .diagnose_document(
                &params.text_document.text,
                params.text_document.uri,
                Status::Unknown,
                None,
                &mut documents,
                None,
                None,
            )
            .await;

        self.commit_document(current_path, document, &mut documents)
            .await;
    }

    async fn did_change(&self, params: tower_lsp::lsp_types::DidChangeTextDocumentParams) {
        let Some(change) = params.content_changes.iter().next() else {
            return;
        };

        let mut documents = self.documents.lock().await;

        let (current_path, document) = self
            .diagnose_document(
                &change.text,
                params.text_document.uri,
                Status::Unknown,
                None,
                &mut documents,
                None,
                None,
            )
            .await;

        self.commit_document(current_path, document, &mut documents)
            .await;
    }

    async fn did_delete_files(&self, params: DeleteFilesParams) {
        for file in params.files {
            let uri_str = file.uri;

            let path = if uri_str.starts_with("file://") {
                PathBuf::from(&uri_str[7..])
            } else {
                continue;
            };

            self.documents.lock().await.remove(&path);

            if let Some(workspace_mutex) = self
                .workspace_for_path(&path, &mut self.workspaces.lock().await)
                .await
            {
                let mut workspace = workspace_mutex.lock().await;

                let Some(luaurc) = &mut workspace.luaurc else {
                    break;
                };

                luaurc.dependants.remove_by_right(path);
            };
        }
    }

    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        let mut workspaces = self.workspaces.lock().await;

        for folder in params.event.added {
            let Ok(workspace_path) = folder.uri.to_file_path() else {
                continue;
            };

            let workspace = Workspace::new(resolve_luaurc(&workspace_path).await);
            workspaces.insert(workspace_path.clone(), Arc::new(Mutex::new(workspace)));
        }

        for folder in params.event.removed {
            let Ok(workspace_path) = folder.uri.to_file_path() else {
                continue;
            };

            workspaces.remove(&workspace_path);
        }
    }

    async fn did_change_watched_files(
        &self,
        params: tower_lsp::lsp_types::DidChangeWatchedFilesParams,
    ) {
        for change in params.changes {
            let Ok(path) = change.uri.to_file_path() else {
                return;
            };

            if !path.is_file() {
                return;
            };

            let prefix = path.file_prefix();

            if !(prefix == Some(OsStr::new("luaurc"))
                || prefix == Some(OsStr::new(".luaurc"))
                || path.extension() == Some(OsStr::new("luaurc")))
            {
                return;
            }

            let Ok(workspace_path) = path.join("../").canonicalize() else {
                return;
            };

            match change.typ {
                FileChangeType::CHANGED => {
                    let workspaces_mutex = self.workspaces.clone();
                    let workspaces = workspaces_mutex.lock().await;

                    let Some(workspace_mutex) = workspaces.get(&workspace_path) else {
                        return;
                    };
                    let mut workspace = workspace_mutex.lock().await;

                    let Some(old_luaurc) = workspace.luaurc.take() else {
                        return;
                    };

                    let old_aliases = old_luaurc.aliases;
                    let fresh_luaurc = Luaurc::from_path(&path).await;

                    let luaurc_alias_diff =
                        old_aliases.diff(&fresh_luaurc.aliases).cloned().collect::<Vec<_>>();

                    let new_luaurc = workspace.luaurc.insert(Luaurc {
                        aliases: fresh_luaurc.aliases,
                        dependants: old_luaurc.dependants,
                        language_mode: fresh_luaurc.language_mode,
                    });

                    let mut documents = self.documents.lock().await;

                    for changed_alias in luaurc_alias_diff {
                        let Some(changed_dependencies) =
                            new_luaurc.dependants.get_by_left(&changed_alias)
                        else {
                            continue;
                        };
                        let changed_dependencies =
                            changed_dependencies.iter().cloned().collect::<Vec<_>>();

                        for document_path in changed_dependencies {
                            let Some(document_mutex) = documents.get(document_path.as_ref()) else {
                                continue;
                            };
                            let document_mutex = document_mutex.clone();
                            let document = document_mutex.lock().await;

                            let Ok(uri) = Url::from_file_path(document_path.as_ref()) else {
                                continue;
                            };

                            let (_, new_document) = self
                                .diagnose_document(
                                    &document.source,
                                    uri,
                                    Status::Some(new_luaurc),
                                    Some(&workspaces),
                                    &mut documents,
                                    Some(&document),
                                    None,
                                )
                                .await;

                            // We can't do `*document = new_document;` as `document` may no longer exist in the map.
                            documents.insert(
                                document_path.as_ref().to_path_buf(),
                                Arc::new(Mutex::new(new_document)),
                            );
                        }
                    }
                }

                FileChangeType::CREATED => {
                    self.workspaces
                        .lock()
                        .await
                        .set_luaurc_for_workspace(&workspace_path, Luaurc::from_path(&path).await)
                        .await;
                }

                FileChangeType::DELETED => {
                    let workspaces_mutex = self.workspaces.clone();
                    let workspaces = workspaces_mutex.lock().await;

                    let Some(workspace_mutex) = workspaces.get(&workspace_path) else {
                        return;
                    };
                    let mut workspace = workspace_mutex.lock().await;

                    let Some(deleted_luaurc) = workspace.luaurc.take() else {
                        return;
                    };

                    let mut documents = self.documents.lock().await;

                    for alias in deleted_luaurc.aliases.keys() {
                        let Some(changed_dependencies) =
                            deleted_luaurc.dependants.get_by_left(alias)
                        else {
                            continue;
                        };
                        let changed_dependencies =
                            changed_dependencies.iter().cloned().collect::<Vec<_>>();

                        for document_path in changed_dependencies {
                            let Some(document_mutex) = documents.get(document_path.as_ref()) else {
                                continue;
                            };
                            let document_mutex = document_mutex.clone();
                            let document = document_mutex.lock().await;

                            let Ok(uri) = Url::from_file_path(document_path.as_ref()) else {
                                continue;
                            };

                            let (_, new_document) = self
                                .diagnose_document(
                                    &document.source,
                                    uri,
                                    Status::None,
                                    Some(&workspaces),
                                    &mut documents,
                                    Some(&document),
                                    None,
                                )
                                .await;

                            // We can't do `*document = new_document;` as `document` may no longer exist in the map.
                            documents.insert(
                                document_path.as_ref().to_path_buf(),
                                Arc::new(Mutex::new(new_document)),
                            );
                        }
                    }
                }

                _ => (),
            };
        }
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> LspResult<Option<GotoDefinitionResponse>> {
        let position = params.text_document_position_params.position;
        let uri = params.text_document_position_params.text_document.uri;

        let Ok(current_path) = uri.to_file_path() else {
            return Ok(None);
        };

        let documents = self.documents.lock().await;

        let Some(document_mutex) = documents.get(&current_path) else {
            return Ok(None);
        };

        let document_mutex = document_mutex.clone();
        let document = document_mutex.lock().await;

        let byte_offset = position.byte_offset(&document.source);

        if let Some((span, kind)) = document.definitions.get_key_value(&byte_offset) {
            let rope = Rope::from_str(&document.source);

            match kind {
                DefinitionKind::Derive { path } => {
                    if let Ok(target_uri) = Url::from_file_path(path) {
                        return Ok(Some(GotoDefinitionResponse::Link(vec![LocationLink {
                            origin_selection_range: Some(convert_range(
                                rbx_rsml::types::Range::from_span(
                                    &rope,
                                    (*span.start(), *span.end()),
                                ),
                            )),
                            target_uri,
                            target_range: Range {
                                start: Position {
                                    line: 0,
                                    character: 0,
                                },
                                end: Position {
                                    line: 0,
                                    character: 0,
                                },
                            },
                            target_selection_range: Range {
                                start: Position {
                                    line: 0,
                                    character: 0,
                                },
                                end: Position {
                                    line: 0,
                                    character: 0,
                                },
                            },
                        }])));
                    }
                }

                DefinitionKind::Selector { .. }
                | DefinitionKind::Scope { .. }
                | DefinitionKind::Assignment { .. }
                | DefinitionKind::EnumName
                | DefinitionKind::EnumVariant { .. }
                | DefinitionKind::Declaration
                | DefinitionKind::FilteredEnumName { .. }
                | DefinitionKind::Token { .. } => (),
            }
        }

        return Ok(None);
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>, tower_lsp::jsonrpc::Error> {
        let position = params.text_document_position_params.position;
        let uri = params.text_document_position_params.text_document.uri;

        let Ok(current_path) = uri.to_file_path() else {
            return Ok(None);
        };

        let documents = self.documents.lock().await;

        let Some(document_mutex) = documents.get(&current_path) else {
            return Ok(None);
        };

        let document_mutex = document_mutex.clone();
        let document = document_mutex.lock().await;

        let byte_offset = position.byte_offset(&document.source);

        if let Some((span, kind)) = document.definitions.get_key_value(&byte_offset) {
            let rope = Rope::from_str(&document.source);

            let contents = match kind {
                DefinitionKind::Derive { path } => HoverContents::Scalar(
                    MarkedString::from_markdown(format!("```luau\n{:#?}\n```", path)),
                ),

                DefinitionKind::Selector { hint, .. } => HoverContents::Scalar(
                    MarkedString::from_markdown(format!("```luau\n{}\n```", hint.to_string())),
                ),

                DefinitionKind::Token { name, is_static } => {
                    HoverContents::Scalar(MarkedString::from_markdown(format!(
                        "```luau\n{}\n```",
                        format_token_hint(name, *is_static, &document.resolved_types)
                    )))
                }

                DefinitionKind::Assignment { property_name, .. } => {
                    let Some(dt) = document.resolved_types.get(&ResolvedTypeKey::Property {
                        start: *span.start(),
                    }) else {
                        return Ok(None);
                    };

                    let resolved = match dt {
                        Datatype::IncompleteEnumShorthand(_) => {
                            format!("Enum.{}", property_name)
                        }

                        Datatype::Variant(Variant::EnumItem(item)) => {
                            format!("Enum.{}", item.ty)
                        }

                        other => other.type_name(),
                    };

                    HoverContents::Scalar(MarkedString::from_markdown(format!(
                        "```luau\n{}: {}\n```",
                        property_name, resolved,
                    )))
                }

                DefinitionKind::Scope { .. }
                | DefinitionKind::EnumName
                | DefinitionKind::EnumVariant { .. }
                | DefinitionKind::Declaration
                | DefinitionKind::FilteredEnumName { .. } => return Ok(None),
            };

            return Ok(Some(Hover {
                contents,
                range: Some(convert_range(rbx_rsml::types::Range::from_span(
                    &rope,
                    (*span.start(), *span.end()),
                ))),
            }));
        }

        Ok(None)
    }

    async fn completion(&self, params: CompletionParams) -> LspResult<Option<CompletionResponse>> {
        let position = params.text_document_position.position;
        let uri = params.text_document_position.text_document.uri;

        let Ok(current_path) = uri.to_file_path() else {
            return Ok(None);
        };

        let documents = self.documents.lock().await;

        let Some(document_mutex) = documents.get(&current_path) else {
            return Ok(None);
        };

        let document_mutex = document_mutex.clone();
        let document = document_mutex.lock().await;

        let byte_offset = position.byte_offset(&document.source);

        let Some((_, kind)) = document.definitions.get_key_value(&byte_offset) else {
            return Ok(None);
        };

        let items = match kind {
            DefinitionKind::Scope { type_definition } => get_property_completions(type_definition),

            DefinitionKind::Assignment {
                property_name,
                type_definition,
            } => autocomplete::values::get_value_completions(
                &document.source,
                byte_offset,
                type_definition,
                property_name,
            ),

            DefinitionKind::EnumName => get_enum_name_completions(),

            DefinitionKind::FilteredEnumName { enum_name } => {
                vec![CompletionItem {
                    label: enum_name.clone(),
                    kind: Some(CompletionItemKind::ENUM),
                    ..CompletionItem::default()
                }]
            }

            DefinitionKind::EnumVariant { enum_name } => get_enum_variant_completions(enum_name),

            _ => return Ok(None),
        };

        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn code_action(&self, params: CodeActionParams) -> LspResult<Option<CodeActionResponse>> {
        let diagnostics = &params.context.diagnostics;

        let mut code_actions = vec![];

        let uri = params.text_document.uri;

        for diagnostic in diagnostics {
            let data = if let Some(Value::Object(data)) = &diagnostic.data {
                data
            } else {
                continue;
            };

            let closest = if let Some(Value::String(data)) = data.get("closest") {
                data
            } else {
                continue;
            };

            let range_start = if let Some(Value::Object(range_start)) = data.get("range_start") {
                range_start
            } else {
                continue;
            };

            let start_line = if let Some(Value::Number(start_line)) = range_start.get("line") {
                if let Some(start_line) = start_line.as_u64() {
                    start_line as u32
                } else {
                    continue;
                }
            } else {
                continue;
            };

            let start_char = if let Some(Value::Number(start_char)) = range_start.get("char") {
                if let Some(start_char) = start_char.as_u64() {
                    start_char as u32
                } else {
                    continue;
                }
            } else {
                continue;
            };

            let range_end = if let Some(Value::Object(range_end)) = data.get("range_end") {
                range_end
            } else {
                continue;
            };

            let end_line = if let Some(Value::Number(end_line)) = range_end.get("line") {
                if let Some(end_line) = end_line.as_u64() {
                    end_line as u32
                } else {
                    continue;
                }
            } else {
                continue;
            };

            let end_char = if let Some(Value::Number(end_char)) = range_end.get("char") {
                if let Some(end_char) = end_char.as_u64() {
                    end_char as u32
                } else {
                    continue;
                }
            } else {
                continue;
            };

            let edit = WorkspaceEdit {
                changes: Some(rbx_rsml::collection! {
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
        }

        Ok(Some(code_actions))
    }
}

fn get_enum_name_completions() -> Vec<CompletionItem> {
    let Ok(db) = rbx_reflection_database::get() else {
        return vec![];
    };
    db.enums
        .keys()
        .map(|name| CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::ENUM),
            ..CompletionItem::default()
        })
        .collect()
}

fn get_enum_variant_completions(enum_name: &str) -> Vec<CompletionItem> {
    let Ok(db) = rbx_reflection_database::get() else {
        return vec![];
    };
    let Some(enum_desc) = db.enums.get(enum_name) else {
        return vec![];
    };
    enum_desc
        .items
        .keys()
        .map(|variant| CompletionItem {
            label: variant.to_string(),
            kind: Some(CompletionItemKind::ENUM_MEMBER),
            ..CompletionItem::default()
        })
        .collect()
}

static ENUM_NAME_OVERRIDES: phf::Map<&str, &str> = phf_macros::phf_map! {
    "FlexMode" => "UIFlexMode",
    "HorizontalFlex" => "UIFlexAlignment",
    "VerticalFlex" => "UIFlexAlignment",
};

fn get_enum_shorthand_completions(
    class_names: &[String],
    property_name: &str,
) -> Vec<CompletionItem> {
    if let Some(enum_name) = ENUM_NAME_OVERRIDES.get(property_name) {
        return get_enum_variant_completions(enum_name);
    }

    let Ok(db) = rbx_reflection_database::get() else {
        return vec![];
    };
    for class_name in class_names {
        let Some(class_desc) = db.classes.get(class_name.as_str()) else {
            continue;
        };
        for ancestor in db.superclasses_iter(class_desc) {
            let Some(prop_desc) = ancestor.properties.get(property_name) else {
                continue;
            };
            let rbx_reflection::DataType::Enum(enum_name) = &prop_desc.data_type else {
                continue;
            };
            return get_enum_variant_completions(enum_name);
        }
    }

    vec![]
}

fn get_property_completions(class_names: &[String]) -> Vec<CompletionItem> {
    use rbx_reflection::PropertyKind;
    use std::collections::HashMap;

    let Ok(db) = rbx_reflection_database::get() else {
        return vec![];
    };

    let mut class_property_sets: Vec<HashMap<&str, &rbx_reflection::PropertyDescriptor>> =
        Vec::new();

    for class_name in class_names {
        let Some(class_desc) = db.classes.get(class_name.as_str()) else {
            continue;
        };

        let mut props: HashMap<&str, &rbx_reflection::PropertyDescriptor> = HashMap::new();

        for ancestor in db.superclasses_iter(class_desc) {
            for (prop_name, prop_desc) in &ancestor.properties {
                if matches!(prop_desc.kind, PropertyKind::Alias { .. }) {
                    continue;
                }
                if matches!(prop_desc.scriptability, rbx_reflection::Scriptability::None) {
                    continue;
                }
                props.entry(prop_name.as_ref()).or_insert(prop_desc);
            }
        }

        class_property_sets.push(props);
    }

    if class_property_sets.is_empty() {
        return vec![];
    }

    let first_set = &class_property_sets[0];

    first_set
        .iter()
        .filter(|(name, _)| {
            class_property_sets[1..]
                .iter()
                .all(|set| set.contains_key(*name))
        })
        .map(|(name, desc)| {
            let detail = match &desc.data_type {
                rbx_reflection::DataType::Value(variant_type) => format!("{:?}", variant_type),
                rbx_reflection::DataType::Enum(enum_name) => format!("Enum.{}", enum_name),
                _ => String::new(),
            };

            CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::PROPERTY),
                detail: Some(detail),
                ..CompletionItem::default()
            }
        })
        .collect()
}

impl<'a> Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            workspaces: Arc::new(Mutex::new(Workspaces::new())),
            documents: Arc::new(Mutex::new(Documents::new())),
        }
    }

    async fn workspace_for_path(
        &self,
        path: &Path,
        workspaces: &MutexGuard<'_, Workspaces>,
    ) -> Option<Arc<Mutex<Workspace>>> {
        let Some((_, workspace)) = workspaces.iter().find(|(x, _)| path.starts_with(x)) else {
            return None;
        };

        let workspace = workspace.clone();

        Some(workspace)
    }

    async fn workspace_path_for_path(
        &'a self,
        path: &'a Path,
        workspaces: &'a MutexGuard<'a, Workspaces>,
    ) -> Option<&'a PathBuf> {
        workspaces
            .iter()
            .find_map(|(x, _)| if path.starts_with(x) { Some(x) } else { None })
    }

    async fn notify(&self, ty: MessageType, msg: String) {
        self.client
            .send_notification::<ShowMessage>(ShowMessageParams {
                typ: ty,
                message: msg,
            })
            .await;
    }

    async fn commit_document(
        &self,
        current_path: Option<PathBuf>,
        document: Document,
        documents: &mut Documents,
    ) {
        if let Some(current_path) = current_path {
            documents.insert(current_path, Arc::new(Mutex::new(document)));
        }
    }

    fn diagnose_document<'b: 'a>(
        &'b self,
        source_code: &'a str,
        uri: Url,
        mut luaurc: Status<&'a mut Luaurc>,
        workspaces: Option<&'a MutexGuard<'b, Workspaces>>,
        documents: &'a mut MutexGuard<'b, Documents>,

        old_document: Option<&'a Document>,

        // If diagnose_document was initiated by another file.
        // Used to prevent infinite loops.
        initiation_path: Option<PathBuf>,
    ) -> Pin<Box<dyn Future<Output = (Option<PathBuf>, Document)> + 'a + Send>> {
        Box::pin(async move {
            let mut document = Document::new(source_code.to_string());

            let (diagnostics, current_path) = if source_code.len() == 0 {
                (vec![], None)
            } else {
                let parsed = RsmlParser::new(RsmlLexer::new(source_code));

                let uri_file_path = uri.to_file_path();

                let (type_errors, current_path) = match uri_file_path {
                    Ok(current_path) => {
                        let typechecked = match luaurc.as_deref_mut() {
                            Status::Some(luaurc) => {
                                Typechecker::new(&parsed, &current_path, Some(luaurc)).await
                            }

                            Status::Unknown => 'outer: {
                                'inner: {
                                    let workspaces = match workspaces {
                                        Some(workspaces) => workspaces,
                                        None => &self.workspaces.lock().await,
                                    };

                                    let Some(workspace_mutex) =
                                        self.workspace_for_path(&current_path, workspaces).await
                                    else {
                                        break 'inner;
                                    };

                                    let mut workspace = workspace_mutex.lock().await;

                                    let Some(luaurc) = &mut workspace.luaurc else {
                                        break 'inner;
                                    };

                                    break 'outer Typechecker::new(
                                        &parsed,
                                        &current_path,
                                        Some(luaurc),
                                    )
                                    .await;
                                }

                                Typechecker::new(&parsed, &current_path, None).await
                            }

                            Status::None => Typechecker::new(&parsed, &current_path, None).await,
                        };

                        // We need to update all documents that this one depends on.
                        // TODO: Reimplement this to a more sophisticated method that
                        // doesn't completely re-diagnose each dependant.

                        let TypecheckedRsml {
                            mut errors,
                            derives,
                            dependencies: type_dependencies,
                            mut definitions,
                            resolved_types,
                        } = typechecked;
                        document.dependencies = type_dependencies;
                        document.resolved_types = resolved_types;

                        // Build autocomplete definitions on top of typechecker's Selector/Scope
                        match &luaurc {
                            Status::Some(luaurc_ref) => {
                                autocomplete::build_definitions(
                                    &parsed,
                                    &mut definitions,
                                    &current_path,
                                    Some(luaurc_ref),
                                );
                            }
                            Status::None => {
                                autocomplete::build_definitions(
                                    &parsed,
                                    &mut definitions,
                                    &current_path,
                                    None,
                                );
                            }
                            Status::Unknown => {
                                let workspaces_for_ac = match workspaces {
                                    Some(ws) => ws,
                                    None => &self.workspaces.lock().await,
                                };
                                if let Some(ws_mutex) = self
                                    .workspace_for_path(&current_path, workspaces_for_ac)
                                    .await
                                {
                                    let workspace = ws_mutex.lock().await;
                                    autocomplete::build_definitions(
                                        &parsed,
                                        &mut definitions,
                                        &current_path,
                                        workspace.luaurc.as_ref(),
                                    );
                                } else {
                                    autocomplete::build_definitions(
                                        &parsed,
                                        &mut definitions,
                                        &current_path,
                                        None,
                                    );
                                }
                            }
                        }

                        document.definitions = definitions;

                        let mut dependencies = document.dependencies.clone();

                        // We also need to update any old dependencies now that this file doesn't depend on it.
                        // (This is largly for updating dependencies that are no longer cyclic).
                        if let Some(old_document) = old_document {
                            dependencies.extend(old_document.dependencies.iter().cloned());
                        } else {
                            if let Some(old_document_mutex) = documents.get(&current_path) {
                                let old_document = old_document_mutex.lock().await;

                                dependencies.extend(old_document.dependencies.iter().cloned());
                            };
                        };

                        // We need to insert the document so that the dependant docs update properly.
                        documents.insert(current_path.clone(), Arc::new(Mutex::new(document)));

                        let workspaces = if let Some(workspaces) = workspaces {
                            workspaces
                        } else {
                            &self.workspaces.lock().await
                        };

                        let workspace_path_for_current_path = self
                            .workspace_path_for_path(&current_path, workspaces)
                            .await;

                        if let Some(initiation_path) = &initiation_path {
                            for dependant_path in dependencies {
                                if initiation_path == &dependant_path {
                                    continue;
                                };

                                self.diagnose_sub_document(
                                    &current_path,
                                    workspace_path_for_current_path,
                                    dependant_path,
                                    workspaces,
                                    documents,
                                    luaurc.as_deref_mut(),
                                )
                                .await;
                            }
                        } else {
                            for dependant_path in dependencies {
                                self.diagnose_sub_document(
                                    &current_path,
                                    workspace_path_for_current_path,
                                    dependant_path,
                                    workspaces,
                                    documents,
                                    luaurc.as_deref_mut(),
                                )
                                .await;
                            }
                        }

                        // We get the document back out.
                        let document_option = 'block: {
                            let Some(document_mutex) = documents.remove(&current_path) else {
                                break 'block None;
                            };

                            let Ok(document_lock) = Arc::try_unwrap(document_mutex) else {
                                break 'block None;
                            };
                            document_lock.into_inner().into()
                        };
                        // This *should* always be Some unless something's gone terribly wrong.
                        document = document_option.unwrap();

                        // Checks for cyclic dependencies.

                        let gathered_dependencies =
                            gather_dependencies(&current_path, &document, documents).await;

                        for (cyclic_path, ancestry_chain) in
                            &gathered_dependencies.cyclic_dependencies
                        {
                            let Some(span) = derives.get(cyclic_path) else {
                                continue;
                            };

                            errors.report(
                                TypeError::CyclicDerive {
                                    kind: CyclicKind::External(ancestry_chain),
                                },
                                parsed.range_from_span((*span.start(), *span.end())),
                            );
                        }

                        (errors, Some(current_path))
                    }

                    Err(_) => {
                        self.notify(
                            MessageType::WARNING,
                            String::from("Could not get the path for the current files. You may experience issues with derives being resolved.")
                        ).await;

                        {
                            let dummy_path = PathBuf::from("/");
                            let TypecheckedRsml {
                                errors,
                                derives: _,
                                dependencies: type_dependencies,
                                mut definitions,
                                resolved_types,
                            } = Typechecker::new(&parsed, &dummy_path, None).await;
                            document.dependencies = type_dependencies;
                            document.resolved_types = resolved_types;
                            autocomplete::build_definitions(
                                &parsed,
                                &mut definitions,
                                &dummy_path,
                                None,
                            );
                            document.definitions = definitions;
                            (errors, None)
                        }
                    }
                };

                let mut all_errors = parsed.ast_errors.0;
                all_errors.extend(type_errors.0);

                (
                    all_errors.into_iter().map(convert_diagnostic).collect(),
                    current_path,
                )
            };

            self.client
                .publish_diagnostics(uri, diagnostics, None)
                .await;

            (current_path, document)
        })
    }

    async fn diagnose_sub_document<'b: 'a>(
        &'b self,
        current_path: &PathBuf,
        workspace_path_for_current_path: Option<&PathBuf>,
        dependant_path: PathBuf,
        workspaces: &'a MutexGuard<'b, Workspaces>,
        documents: &'a mut MutexGuard<'b, Documents>,
        luaurc: Status<&'a mut Luaurc>,
    ) {
        let Ok(dependant_uri) = Url::from_file_path(&dependant_path) else {
            return;
        };

        let dependant_source = match documents.get(&dependant_path) {
            Some(document) => document.lock().await.source.to_string(),
            None => {
                let Ok(source) = fs::read_to_string(&dependant_path).await else {
                    return;
                };
                source
            }
        };

        let workspace_path_for_dependant_path = self
            .workspace_path_for_path(&dependant_path, workspaces)
            .await;

        let next_luaurc = if workspace_path_for_current_path == workspace_path_for_dependant_path {
            luaurc
        } else {
            Status::Unknown
        };

        let (_, document) = self
            .diagnose_document(
                &dependant_source,
                dependant_uri,
                next_luaurc,
                Some(workspaces),
                documents,
                None,
                Some(current_path.clone()),
            )
            .await;

        documents.insert(dependant_path, Arc::new(Mutex::new(document)));
    }

    fn populate_workspace<'b: 'a>(
        &'b self,
        entry_path: PathBuf,
        workspaces: &'a MutexGuard<'b, Workspaces>,
        documents: &'a mut MutexGuard<'b, Documents>,
    ) -> Pin<Box<dyn Future<Output = ()> + 'a + Send>> {
        Box::pin(async move {
            let Ok(mut dir) = fs::read_dir(&entry_path).await else {
                return;
            };

            while let Ok(Some(entry)) = dir.next_entry().await {
                let path = entry.path();

                if path.is_file() && path.extension() == Some(OsStr::new("rsml")) {
                    let Ok(uri) = Url::from_file_path(&path) else {
                        continue;
                    };
                    let Ok(source) = fs::read_to_string(&path).await else {
                        continue;
                    };

                    let (current_path, document) = self
                        .diagnose_document(
                            &source,
                            uri,
                            Status::Unknown,
                            Some(workspaces),
                            documents,
                            None,
                            None,
                        )
                        .await;

                    self.commit_document(current_path, document, documents)
                        .await;
                } else if path.is_dir() {
                    self.populate_workspace(path, workspaces, documents).await;
                }
            }
        })
    }
}

async fn watch() {
    let (stdin, stdout) = (tokio::io::stdin(), tokio::io::stdout());

    let (service, socket) = LspService::new(Backend::new);

    Server::new(stdin, stdout, socket).serve(service).await;
}

async fn test() {
    let contents = fs::read_to_string("./test/test.rsml").await.unwrap();

    let lexed = RsmlLexer::new(&contents);
    println!("{:#?}", lexed.collect::<Vec<SpannedToken>>());

    let parsed = RsmlParser::new(RsmlLexer::new(&contents));

    let typechecked = Typechecker::new(&parsed, &PathBuf::from("/"), None).await;

    println!(
        "{:#?} {:#?} {:#?}",
        parsed.ast, parsed.ast_errors, typechecked.errors
    );
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

#[derive(Debug)]
pub struct GatheredDependencies {
    pub dependencies: HashSet<PathBuf>,
    pub cyclic_dependencies: HashSet<(PathBuf, String)>,
}

async fn gather_dependencies(
    current_path: &Path,
    document: &Document,
    documents: &MutexGuard<'_, Documents>,
) -> GatheredDependencies {
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut cyclic_dependencies: HashSet<(PathBuf, String)> = HashSet::new();
    let mut queue: VecDeque<(PathBuf, Vec<PathBuf>)> = VecDeque::new();

    visited.insert(current_path.to_path_buf());
    queue.push_back((current_path.to_path_buf(), vec![current_path.to_path_buf()]));

    while let Some((node, ancestors)) = queue.pop_front() {
        let this_document = if node == current_path {
            document
        } else {
            let Some(document_mutex) = documents.get(&node) else {
                continue;
            };
            &document_mutex.lock().await
        };

        let neighbours = &this_document.dependencies;

        for neighbour in neighbours {
            if ancestors.contains(&neighbour) {
                // We get the second ancestor as its the stylesheet that is being derived.
                if let Some(cyclic_dependency) = ancestors.get(1) {
                    let ancestry_chain = ancestors
                        .iter()
                        .map(|x| format!("{:#?}", x))
                        .intersperse(" -> ".to_string())
                        .collect::<String>();

                    cyclic_dependencies.insert((cyclic_dependency.to_path_buf(), ancestry_chain));
                }
            }

            if !visited.contains(neighbour.as_path()) {
                let mut new_ancestors = ancestors.clone();
                new_ancestors.push(neighbour.to_path_buf());

                queue.push_back((neighbour.to_path_buf(), new_ancestors));
            }
        }

        if node != current_path {
            visited.insert(node);
        }
    }

    GatheredDependencies {
        dependencies: visited,
        cyclic_dependencies,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::CompletionItemKind;

    #[test]
    fn completions_for_frame_include_size() {
        let items = get_property_completions(&vec!["Frame".to_string()]);
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert!(labels.contains(&"Size"));
        assert!(labels.contains(&"BackgroundColor3"));
        assert!(labels.contains(&"Name"));
    }

    #[test]
    fn completions_have_property_kind() {
        let items = get_property_completions(&vec!["Frame".to_string()]);
        assert!(
            items
                .iter()
                .all(|item| item.kind == Some(CompletionItemKind::PROPERTY))
        );
    }

    #[test]
    fn completions_intersection_for_union_types() {
        let frame_items = get_property_completions(&vec!["Frame".to_string()]);
        let union_items =
            get_property_completions(&vec!["Frame".to_string(), "TextButton".to_string()]);
        assert!(union_items.len() <= frame_items.len());
        let union_labels: Vec<&str> = union_items.iter().map(|item| item.label.as_str()).collect();
        assert!(union_labels.contains(&"Name"));
    }

    #[test]
    fn completions_empty_for_unknown_class() {
        let items = get_property_completions(&vec!["NotARealClass".to_string()]);
        assert!(items.is_empty());
    }

    #[test]
    fn completions_empty_for_empty_input() {
        let items = get_property_completions(&vec![]);
        assert!(items.is_empty());
    }

    #[test]
    fn enum_name_completions_not_empty() {
        let items = get_enum_name_completions();
        assert!(!items.is_empty());
        assert!(
            items
                .iter()
                .all(|item| item.kind == Some(CompletionItemKind::ENUM))
        );
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert!(labels.contains(&"AutomaticSize"));
    }

    #[test]
    fn enum_variant_completions() {
        let items = get_enum_variant_completions("AutomaticSize");
        assert!(!items.is_empty());
        assert!(
            items
                .iter()
                .all(|item| item.kind == Some(CompletionItemKind::ENUM_MEMBER))
        );
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert!(labels.contains(&"XY"));
    }

    #[test]
    fn enum_variant_completions_unknown_enum() {
        let items = get_enum_variant_completions("NotAnEnum");
        assert!(items.is_empty());
    }

    #[test]
    fn enum_shorthand_infers_from_property() {
        let items = get_enum_shorthand_completions(&vec!["Frame".to_string()], "AutomaticSize");
        assert!(!items.is_empty());
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert!(labels.contains(&"XY"));
    }

    #[test]
    fn enum_shorthand_override_flex_mode() {
        let items = get_enum_shorthand_completions(&vec!["UIFlexItem".to_string()], "FlexMode");
        assert!(!items.is_empty());
        // Should use UIFlexMode enum, not FlexMode
        let items_from_override = get_enum_variant_completions("UIFlexMode");
        assert_eq!(items.len(), items_from_override.len());
    }

    #[test]
    fn enum_shorthand_scale_type_on_image_label() {
        let items = get_enum_shorthand_completions(&vec!["ImageLabel".to_string()], "ScaleType");
        assert!(
            !items.is_empty(),
            "ScaleType enum shorthand should return variants for ImageLabel"
        );
    }

    #[test]
    fn enum_shorthand_non_enum_property_returns_empty() {
        let items = get_enum_shorthand_completions(&vec!["Frame".to_string()], "Size");
        assert!(items.is_empty());
    }

    #[test]
    fn enum_shorthand_unknown_property_returns_empty() {
        let items = get_enum_shorthand_completions(&vec!["Frame".to_string()], "NotARealProperty");
        assert!(items.is_empty());
    }

    async fn typecheck_and_get_definitions(source: &str) -> Document {
        use rbx_rsml::lexer::RsmlLexer;
        use rbx_rsml::parser::RsmlParser;
        use rbx_rsml::typechecker::Typechecker;
        use std::path::PathBuf;

        let lexer = RsmlLexer::new(source);
        let parsed = RsmlParser::new(lexer);
        let dummy_path = PathBuf::from("/test.rsml");
        let data = Typechecker::new(&parsed, &dummy_path, None).await;
        let mut document = Document::new(source.to_string());
        document.dependencies = data.dependencies;
        document.resolved_types = data.resolved_types;
        let mut definitions = data.definitions;
        crate::autocomplete::build_definitions(&parsed, &mut definitions, &dummy_path, None);
        document.definitions = definitions;
        document
    }

    #[tokio::test]
    async fn definitions_trailing_colon_stays_inside_assignment() {
        let source = "Frame {\n    BackgroundColor3 = tw:\n}";
        let document = typecheck_and_get_definitions(source).await;

        let colon_pos = source.find("tw:").unwrap() + 2;
        let after_colon = colon_pos + 1;

        for byte_pos in [colon_pos, after_colon] {
            let entry = document.definitions.get_key_value(&byte_pos);
            assert!(entry.is_some(), "should have entry at byte {}", byte_pos);

            match entry.unwrap().1 {
                DefinitionKind::Assignment { property_name, .. } => {
                    assert_eq!(property_name, "BackgroundColor3");
                }
                other => panic!(
                    "expected Assignment at byte {} (trailing `:` after tw), got {:?}",
                    byte_pos,
                    std::mem::discriminant(other)
                ),
            }
        }
    }

    #[tokio::test]
    async fn definitions_trailing_colon_after_tailwind_token_stays_inside_assignment() {
        // `tw:amber` lexes as one TailwindColor token, so the trailing `:`
        // is recovered as a phantom Rule *after* the Assignment's captured
        // RHS. Without the phantom-Rule absorption in selectors.rs, the
        // cursor one byte past the trailing `:` (where the client queries)
        // falls into the Selector range and completion returns nothing.
        let source = "Frame {\n    BackgroundColor3 = tw:amber:\n}";
        let document = typecheck_and_get_definitions(source).await;

        let trailing_colon = source.rfind(':').unwrap();
        let after_colon = trailing_colon + 1;

        for byte_pos in [trailing_colon, after_colon] {
            let entry = document.definitions.get_key_value(&byte_pos);
            assert!(entry.is_some(), "should have entry at byte {}", byte_pos);

            match entry.unwrap().1 {
                DefinitionKind::Assignment { property_name, .. } => {
                    assert_eq!(property_name, "BackgroundColor3");
                }
                other => panic!(
                    "expected Assignment at byte {} (trailing `:` after tw:amber), got {:?}",
                    byte_pos,
                    std::mem::discriminant(other)
                ),
            }
        }
    }

    #[tokio::test]
    async fn definitions_assignment_with_unparseable_rhs_does_not_leak_to_scope() {
        let source = "ImageButton {\n    BackgroundColor3 = ff\n}";
        let document = typecheck_and_get_definitions(source).await;

        let ff_start = source.find("ff").unwrap();

        for byte_pos in [ff_start, ff_start + 1, ff_start + 2] {
            let entry = document.definitions.get_key_value(&byte_pos);
            assert!(entry.is_some(), "should have entry at byte {}", byte_pos);
            match entry.unwrap().1 {
                DefinitionKind::Assignment { property_name, .. } => {
                    assert_eq!(property_name, "BackgroundColor3");
                }
                other => panic!(
                    "expected Assignment at byte {} (within unparseable RHS), got {:?}",
                    byte_pos,
                    std::mem::discriminant(other)
                ),
            }
        }
    }

    #[tokio::test]
    async fn definitions_shorthand_colon() {
        let source = "ImageLabel :hover {\n    ScaleType = :\n}";
        let document = typecheck_and_get_definitions(source).await;

        let on_colon = source.find("= :").unwrap() + 2;
        let after_colon = on_colon + 1;

        for byte_pos in [on_colon, after_colon] {
            let entry = document.definitions.get_key_value(&byte_pos);
            assert!(entry.is_some(), "should have entry at byte {}", byte_pos);
            match entry.unwrap().1 {
                DefinitionKind::Assignment {
                    property_name,
                    type_definition,
                } => {
                    assert_eq!(property_name, "ScaleType");
                    assert!(type_definition.contains(&"ImageLabel".to_string()));
                }
                other => panic!(
                    "expected Assignment at byte {}, got {:?}",
                    byte_pos,
                    std::mem::discriminant(other)
                ),
            }
        }
    }

    #[tokio::test]
    async fn definitions_enum_dot_name_completions() {
        let source = "Frame {\n    AutomaticSize = Enum.\n}";
        let document = typecheck_and_get_definitions(source).await;

        let dot_pos = source.find("Enum.").unwrap() + 5;
        let entry = document.definitions.get_key_value(&dot_pos);
        assert!(
            entry.is_some(),
            "should have entry at byte {} (after 'Enum.')",
            dot_pos
        );
        match entry.unwrap().1 {
            DefinitionKind::FilteredEnumName { enum_name } => {
                assert_eq!(enum_name, "AutomaticSize");
            }
            other => panic!(
                "expected FilteredEnumName at byte {}, got {:?}",
                dot_pos,
                std::mem::discriminant(other)
            ),
        }
    }

    #[tokio::test]
    async fn definitions_enum_colon_name_completions() {
        let source = "Frame {\n    AutomaticSize = Enum:\n}";
        let document = typecheck_and_get_definitions(source).await;

        let colon_pos = source.find("Enum:").unwrap() + 5;
        let entry = document.definitions.get_key_value(&colon_pos);
        assert!(
            entry.is_some(),
            "should have entry at byte {} (after 'Enum:')",
            colon_pos
        );
        match entry.unwrap().1 {
            DefinitionKind::FilteredEnumName { enum_name } => {
                assert_eq!(enum_name, "AutomaticSize");
            }
            other => panic!(
                "expected FilteredEnumName at byte {}, got {:?}",
                colon_pos,
                std::mem::discriminant(other)
            ),
        }
    }

    #[tokio::test]
    async fn definitions_enum_dot_name_non_enum_property_falls_back_to_enum_name() {
        let source = "Frame {\n    Size = Enum.\n}";
        let document = typecheck_and_get_definitions(source).await;

        let dot_pos = source.find("Enum.").unwrap() + 5;
        let entry = document.definitions.get_key_value(&dot_pos);
        assert!(
            entry.is_some(),
            "should have entry at byte {} (after 'Enum.')",
            dot_pos
        );
        assert!(
            matches!(entry.unwrap().1, DefinitionKind::EnumName),
            "expected EnumName (unfiltered) for non-enum property"
        );
    }

    #[tokio::test]
    async fn definitions_enum_variant_overrides_wrong_typed_name() {
        // Even if user typed the wrong enum name, variants should still be
        // filtered by the property's declared enum.
        let source = "Frame {\n    AutomaticSize = Enum.Foo.\n}";
        let document = typecheck_and_get_definitions(source).await;

        let trailing_dot = source.rfind('.').unwrap() + 1;
        let entry = document.definitions.get_key_value(&trailing_dot);
        assert!(entry.is_some());
        match entry.unwrap().1 {
            DefinitionKind::EnumVariant { enum_name } => {
                assert_eq!(enum_name, "AutomaticSize");
            }
            other => panic!(
                "expected EnumVariant with AutomaticSize, got {:?}",
                std::mem::discriminant(other)
            ),
        }
    }

    #[tokio::test]
    async fn definitions_enum_variant_after_name_dot() {
        let source = "Frame {\n    AutomaticSize = Enum.AutomaticSize.\n}";
        let document = typecheck_and_get_definitions(source).await;

        let trailing_dot = source.find("AutomaticSize.").unwrap() + 14;
        let entry = document.definitions.get_key_value(&trailing_dot);
        assert!(
            entry.is_some(),
            "should have entry at byte {} (after 'Enum.AutomaticSize.')",
            trailing_dot
        );
        match entry.unwrap().1 {
            DefinitionKind::EnumVariant { enum_name } => {
                assert_eq!(enum_name, "AutomaticSize");
            }
            other => panic!(
                "expected EnumVariant at byte {}, got {:?}",
                trailing_dot,
                std::mem::discriminant(other)
            ),
        }
    }

    #[tokio::test]
    async fn definitions_enum_variant_after_name_colon() {
        let source = "Frame {\n    AutomaticSize = Enum:AutomaticSize:\n}";
        let document = typecheck_and_get_definitions(source).await;

        let trailing_colon = source.rfind(':').unwrap() + 1;
        let entry = document.definitions.get_key_value(&trailing_colon);
        assert!(
            entry.is_some(),
            "should have entry at byte {} (after 'Enum:AutomaticSize:')",
            trailing_colon
        );
        match entry.unwrap().1 {
            DefinitionKind::EnumVariant { enum_name } => {
                assert_eq!(enum_name, "AutomaticSize");
            }
            other => panic!(
                "expected EnumVariant at byte {}, got {:?}",
                trailing_colon,
                std::mem::discriminant(other)
            ),
        }
    }

    #[tokio::test]
    async fn definitions_non_enum_assignment_returns_empty_completions() {
        let source = "Frame {\n    Size = \n}";
        let document = typecheck_and_get_definitions(source).await;

        let after_equals = source.find("= ").unwrap() + 2;
        let entry = document.definitions.get_key_value(&after_equals);
        assert!(
            entry.is_some(),
            "should have entry at byte {}",
            after_equals
        );
        match entry.unwrap().1 {
            DefinitionKind::Assignment {
                property_name,
                type_definition,
            } => {
                let items = get_enum_shorthand_completions(type_definition, property_name);
                assert!(
                    items.is_empty(),
                    "Size is not an enum, should return empty completions"
                );
            }
            _ => (),
        }
    }

    #[tokio::test]
    async fn tween_arg1_has_easing_style_variant_definition() {
        let source = "Frame {\n    @tween Size (.5, :Linear);\n}";
        let document = typecheck_and_get_definitions(source).await;

        let colon_pos = source.rfind(":Linear").unwrap();
        let entry = document.definitions.get_key_value(&colon_pos);
        assert!(
            entry.is_some(),
            "should have EnumVariant entry at tween arg 1"
        );
        match entry.unwrap().1 {
            DefinitionKind::EnumVariant { enum_name } => {
                assert_eq!(enum_name, "EasingStyle");
            }
            other => panic!(
                "expected EnumVariant at tween arg 1, got {:?}",
                std::mem::discriminant(other)
            ),
        }
    }

    #[tokio::test]
    async fn tween_arg2_has_easing_direction_variant_definition() {
        let source = "Frame {\n    @tween Size (.5, :Linear, :InOut);\n}";
        let document = typecheck_and_get_definitions(source).await;

        let colon_pos = source.rfind(":InOut").unwrap();
        let entry = document.definitions.get_key_value(&colon_pos);
        assert!(
            entry.is_some(),
            "should have EnumVariant entry at tween arg 2"
        );
        match entry.unwrap().1 {
            DefinitionKind::EnumVariant { enum_name } => {
                assert_eq!(enum_name, "EasingDirection");
            }
            other => panic!(
                "expected EnumVariant at tween arg 2, got {:?}",
                std::mem::discriminant(other)
            ),
        }
    }

    #[tokio::test]
    async fn tween_arg1_full_enum_name_shows_filtered_name() {
        let source = "Frame {\n    @tween Size (.5, Enum.EasingStyle.Linear);\n}";
        let document = typecheck_and_get_definitions(source).await;

        // After "Enum" keyword should show only "EasingStyle" (FilteredEnumName)
        let after_enum = source.find("Enum.EasingStyle").unwrap() + 4;
        let entry = document.definitions.get_key_value(&after_enum);
        assert!(entry.is_some());
        match entry.unwrap().1 {
            DefinitionKind::FilteredEnumName { enum_name } => {
                assert_eq!(enum_name, "EasingStyle");
            }
            other => panic!(
                "expected FilteredEnumName, got {:?}",
                std::mem::discriminant(other)
            ),
        }

        // After "EasingStyle" should show variants
        let after_name = source.find("EasingStyle.Linear").unwrap() + 11;
        let entry = document.definitions.get_key_value(&after_name);
        assert!(entry.is_some());
        match entry.unwrap().1 {
            DefinitionKind::EnumVariant { enum_name } => {
                assert_eq!(enum_name, "EasingStyle");
            }
            other => panic!(
                "expected EnumVariant, got {:?}",
                std::mem::discriminant(other)
            ),
        }
    }

    #[tokio::test]
    async fn tween_arg2_full_enum_name_shows_filtered_name() {
        let source = "Frame {\n    @tween Size (.5, Enum.EasingStyle.Linear, Enum.EasingDirection.InOut);\n}";
        let document = typecheck_and_get_definitions(source).await;

        // After "Enum" keyword should show only "EasingDirection" (FilteredEnumName)
        let after_enum = source.find("Enum.EasingDirection").unwrap() + 4;
        let entry = document.definitions.get_key_value(&after_enum);
        assert!(entry.is_some());
        match entry.unwrap().1 {
            DefinitionKind::FilteredEnumName { enum_name } => {
                assert_eq!(enum_name, "EasingDirection");
            }
            other => panic!(
                "expected FilteredEnumName, got {:?}",
                std::mem::discriminant(other)
            ),
        }

        // After "EasingDirection" should show variants
        let after_name = source.find("EasingDirection.InOut").unwrap() + 15;
        let entry = document.definitions.get_key_value(&after_name);
        assert!(entry.is_some());
        match entry.unwrap().1 {
            DefinitionKind::EnumVariant { enum_name } => {
                assert_eq!(enum_name, "EasingDirection");
            }
            other => panic!(
                "expected EnumVariant, got {:?}",
                std::mem::discriminant(other)
            ),
        }
    }

    #[tokio::test]
    async fn declaration_derive_suppresses_scope_completions() {
        let source = "Frame {\n    @derive \"./other.rsml\";\n}";
        let document = typecheck_and_get_definitions(source).await;

        let derive_pos = source.find("@derive").unwrap();
        let entry = document.definitions.get_key_value(&derive_pos);
        assert!(
            entry.is_some(),
            "should have definition at @derive position"
        );
        assert!(
            !matches!(entry.unwrap().1, DefinitionKind::Scope { .. }),
            "declaration should override parent Scope"
        );
    }

    #[tokio::test]
    async fn declaration_priority_suppresses_scope_completions() {
        let source = "Frame {\n    @priority 1;\n}";
        let document = typecheck_and_get_definitions(source).await;

        let priority_pos = source.find("@priority").unwrap();
        let entry = document.definitions.get_key_value(&priority_pos);
        assert!(
            entry.is_some(),
            "should have definition at @priority position"
        );
        assert!(
            matches!(entry.unwrap().1, DefinitionKind::Declaration),
            "expected Declaration at @priority"
        );
    }

    #[tokio::test]
    async fn declaration_tween_arg_suppresses_scope_completions() {
        let source = "Frame {\n    @tween Size (.5, :Linear);\n}";
        let document = typecheck_and_get_definitions(source).await;

        let colon_pos = source.find(":Linear").unwrap();
        let entry = document.definitions.get_key_value(&colon_pos);
        assert!(
            entry.is_some(),
            "should have definition at tween arg position"
        );
        assert!(
            !matches!(entry.unwrap().1, DefinitionKind::Scope { .. }),
            "tween arg should override parent Scope"
        );
    }

    #[tokio::test]
    async fn declaration_macro_header_suppresses_scope_completions() {
        let source = "Frame {\n    @macro Test() {\n        Size = 10;\n    }\n}";
        let document = typecheck_and_get_definitions(source).await;

        let macro_pos = source.find("@macro").unwrap();
        let entry = document.definitions.get_key_value(&macro_pos);
        assert!(entry.is_some(), "should have definition at @macro position");
        assert!(
            matches!(entry.unwrap().1, DefinitionKind::Declaration),
            "expected Declaration at @macro header"
        );
    }

    #[tokio::test]
    async fn declaration_macro_body_allows_scope_completions() {
        let source = "Frame {\n    @macro Test() {\n        Size = 10;\n    }\n}";
        let document = typecheck_and_get_definitions(source).await;

        let size_pos = source.find("Size").unwrap();
        let entry = document.definitions.get_key_value(&size_pos);
        assert!(entry.is_some(), "should have definition inside macro body");
        assert!(
            !matches!(entry.unwrap().1, DefinitionKind::Declaration),
            "macro body should not be Declaration — it needs its own completions"
        );
    }

    // ── Tween completion behavior ─────────────────────────────────

    #[tokio::test]
    async fn tween_shorthand_colon_shows_easing_style_variants() {
        let source = "Frame {\n    @tween Size (.5, :Linear);\n}";
        let document = typecheck_and_get_definitions(source).await;

        let colon_pos = source.find(":Linear").unwrap();
        let entry = document.definitions.get_key_value(&colon_pos);
        assert!(entry.is_some());
        match entry.unwrap().1 {
            DefinitionKind::EnumVariant { enum_name } => {
                assert_eq!(enum_name, "EasingStyle");
                let items = get_enum_variant_completions(enum_name);
                assert!(items.iter().any(|item| item.label == "Linear"));
            }
            other => panic!(
                "expected EnumVariant, got {:?}",
                std::mem::discriminant(other)
            ),
        }
    }

    #[tokio::test]
    async fn tween_shorthand_colon_arg2_shows_easing_direction_variants() {
        let source = "Frame {\n    @tween Size (.5, :Linear, :InOut);\n}";
        let document = typecheck_and_get_definitions(source).await;

        let colon_pos = source.rfind(":InOut").unwrap();
        let entry = document.definitions.get_key_value(&colon_pos);
        assert!(entry.is_some());
        match entry.unwrap().1 {
            DefinitionKind::EnumVariant { enum_name } => {
                assert_eq!(enum_name, "EasingDirection");
                let items = get_enum_variant_completions(enum_name);
                assert!(items.iter().any(|item| item.label == "InOut"));
            }
            other => panic!(
                "expected EnumVariant, got {:?}",
                std::mem::discriminant(other)
            ),
        }
    }

    #[tokio::test]
    async fn tween_enum_dot_shows_only_correct_enum_name() {
        let source = "Frame {\n    @tween Size (.5, Enum.EasingStyle.Linear);\n}";
        let document = typecheck_and_get_definitions(source).await;

        // After "Enum" keyword should be FilteredEnumName with only "EasingStyle"
        let after_enum = source.find("Enum.EasingStyle").unwrap() + 4;
        let entry = document.definitions.get_key_value(&after_enum);
        assert!(entry.is_some());
        match entry.unwrap().1 {
            DefinitionKind::FilteredEnumName { enum_name } => {
                assert_eq!(enum_name, "EasingStyle");
            }
            other => panic!(
                "expected FilteredEnumName, got {:?}",
                std::mem::discriminant(other)
            ),
        }
    }

    #[tokio::test]
    async fn tween_enum_dot_arg2_shows_only_correct_enum_name() {
        let source = "Frame {\n    @tween Size (.5, :Linear, Enum.EasingDirection.InOut);\n}";
        let document = typecheck_and_get_definitions(source).await;

        let after_enum = source.find("Enum.EasingDirection").unwrap() + 4;
        let entry = document.definitions.get_key_value(&after_enum);
        assert!(entry.is_some());
        match entry.unwrap().1 {
            DefinitionKind::FilteredEnumName { enum_name } => {
                assert_eq!(enum_name, "EasingDirection");
            }
            other => panic!(
                "expected FilteredEnumName, got {:?}",
                std::mem::discriminant(other)
            ),
        }
    }

    #[tokio::test]
    async fn tween_enum_name_dot_shows_variants() {
        let source = "Frame {\n    @tween Size (.5, Enum.EasingStyle.Linear);\n}";
        let document = typecheck_and_get_definitions(source).await;

        // After "EasingStyle" should show variants
        let after_name = source.find("EasingStyle.Linear").unwrap() + 11;
        let entry = document.definitions.get_key_value(&after_name);
        assert!(entry.is_some());
        match entry.unwrap().1 {
            DefinitionKind::EnumVariant { enum_name } => {
                assert_eq!(enum_name, "EasingStyle");
            }
            other => panic!(
                "expected EnumVariant, got {:?}",
                std::mem::discriminant(other)
            ),
        }
    }

    fn assert_token_at(
        document: &Document,
        byte_pos: usize,
        expected_name: &str,
        expected_static: bool,
    ) {
        let entry = document.definitions.get_key_value(&byte_pos);
        let entry = entry.unwrap_or_else(|| panic!("no definition entry at byte {}", byte_pos));
        match entry.1 {
            DefinitionKind::Token { name, is_static } => {
                assert_eq!(name, expected_name, "name mismatch at byte {}", byte_pos);
                assert_eq!(
                    *is_static, expected_static,
                    "is_static mismatch at byte {}",
                    byte_pos
                );
            }
            other => panic!(
                "expected Token at byte {}, got {:?}",
                byte_pos,
                std::mem::discriminant(other)
            ),
        }
    }

    #[tokio::test]
    async fn token_lhs_declaration_has_entry() {
        let source = "$primary = #ff0000;";
        let document = typecheck_and_get_definitions(source).await;
        let pos = source.find("$primary").unwrap() + 1;
        assert_token_at(&document, pos, "primary", false);
    }

    #[tokio::test]
    async fn token_static_lhs_declaration_has_entry() {
        let source = "$!primary = #ff0000;";
        let document = typecheck_and_get_definitions(source).await;
        let pos = source.find("$!primary").unwrap() + 2;
        assert_token_at(&document, pos, "primary", true);
    }

    #[tokio::test]
    async fn token_reference_in_assignment_has_entry() {
        let source = "$primary = #ff0000;\nFrame {\n    BackgroundColor3 = $primary;\n}";
        let document = typecheck_and_get_definitions(source).await;
        let ref_start = source.rfind("$primary").unwrap();
        let ref_pos = ref_start + 1;
        assert_token_at(&document, ref_pos, "primary", false);
    }

    #[tokio::test]
    async fn token_reference_inside_math_has_entry() {
        let source = "$a = 10;\n$b = $a + 5;";
        let document = typecheck_and_get_definitions(source).await;
        let ref_pos = source.rfind("$a").unwrap() + 1;
        assert_token_at(&document, ref_pos, "a", false);
    }

    #[tokio::test]
    async fn token_reference_inside_table_has_entry() {
        let source = "$offset = 10;\nFrame {\n    Size = udim2(0, $offset, 0, $offset);\n}";
        let document = typecheck_and_get_definitions(source).await;
        let ref_pos = source.rfind("$offset").unwrap() + 1;
        assert_token_at(&document, ref_pos, "offset", false);
    }

    #[tokio::test]
    async fn token_reference_static_inside_nested_rule() {
        let source = "$!color = #ff0000;\nFrame {\n    BackgroundColor3 = $!color;\n}";
        let document = typecheck_and_get_definitions(source).await;
        let ref_start = source.rfind("$!color").unwrap();
        let ref_pos = ref_start + 2;
        assert_token_at(&document, ref_pos, "color", true);
    }

    async fn hover_for(source: &str, name: &str, is_static: bool) -> String {
        let document = typecheck_and_get_definitions(source).await;
        format_token_hint(name, is_static, &document.resolved_types)
    }

    #[tokio::test]
    async fn hover_dynamic_number() {
        assert_eq!(hover_for("$x = 10;", "x", false).await, "$x: number");
    }

    #[tokio::test]
    async fn hover_static_number() {
        assert_eq!(hover_for("$!x = 10;", "x", true).await, "$!x: number");
    }

    #[tokio::test]
    async fn hover_dynamic_tailwind_coerced_to_color3() {
        assert_eq!(
            hover_for("$x = tw:red:500;", "x", false).await,
            "$x: Color3"
        );
    }

    #[tokio::test]
    async fn hover_static_tailwind_stays_oklab() {
        assert_eq!(
            hover_for("$!x = tw:red:500;", "x", true).await,
            "$!x: Oklab"
        );
    }

    #[tokio::test]
    async fn hover_static_bool_is_boolean() {
        assert_eq!(hover_for("$!x = true;", "x", true).await, "$!x: boolean");
    }

    #[tokio::test]
    async fn hover_enum_shorthand_uses_token_name() {
        assert_eq!(
            hover_for("$!ScaleType = :Fit;", "ScaleType", true).await,
            "$!ScaleType: Enum.ScaleType"
        );
    }

    #[tokio::test]
    async fn hover_full_enum_uses_item_ty() {
        assert_eq!(
            hover_for("$!x = Enum.Material.Plastic;", "x", true).await,
            "$!x: Enum.Material"
        );
    }

    #[tokio::test]
    async fn hover_unresolvable_enum_is_unknown() {
        assert_eq!(
            hover_for("$!x = Enum.NotReal.xyz;", "x", true).await,
            "$!x: unknown"
        );
    }

    #[tokio::test]
    async fn hover_unknown_token_name_is_unknown() {
        let empty_types: ResolvedTypes = std::collections::HashMap::new();
        assert_eq!(
            format_token_hint("missing", false, &empty_types),
            "$missing: unknown"
        );
    }
}
