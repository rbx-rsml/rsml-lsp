#![feature(generic_const_exprs)]

use serde_json::Value;
use tokio::fs;
use tower_lsp::jsonrpc::Result as LspResult;
use tower_lsp::lsp_types::{CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, CodeActionProviderCapability, CodeActionResponse, InitializeParams, InitializeResult, Position, Range, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, TextEdit, Url, WorkspaceEdit};
use tower_lsp::{Client, LanguageServer, LspService, Server};

mod lexer;
use lexer::Lexer;

mod parser;
use parser::Parser;

use crate::lexer::SpannedToken;

mod guarded_unwrap;  

mod list;

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

struct Backend {
    client: Client
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

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> tower_lsp::jsonrpc::Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
                code_action_provider: Some(CodeActionProviderCapability::Simple(true)),
                ..ServerCapabilities::default()
            },
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _: tower_lsp::lsp_types::InitializedParams) {
        //self.client.show_message(tower_lsp::lsp_types::MessageType::INFO, "Rust Tree-sitter LSP initialized!").await;
    }

    async fn shutdown(&self) -> tower_lsp::jsonrpc::Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: tower_lsp::lsp_types::DidOpenTextDocumentParams) {
        self.parse_and_log(&params.text_document.text, params.text_document.uri).await;
    }

    async fn did_change(&self, params: tower_lsp::lsp_types::DidChangeTextDocumentParams) {
        if let Some(change) = params.content_changes.into_iter().next() {
            self.parse_and_log(&change.text, params.text_document.uri).await;
        }
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
    async fn parse_and_log(&self, source_code: &str, uri: Url) {
        let diagnostics = if source_code.len() == 0 {
            vec![]

        } else {
            let parsed = Parser::new(Lexer::new(source_code));
            parsed.ast_errors.0
        };

        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
    }
}

async fn watch() {
    let (stdin, stdout) = (tokio::io::stdin(), tokio::io::stdout());

    let (service, socket) = LspService::new(|client| Backend {
        client
    });

    Server::new(stdin, stdout, socket).serve(service).await;
}

async fn test() {
    let contents = fs::read_to_string("./test.rsml").await.unwrap();
    
    let lexed = Lexer::new(&contents);
    println!("{:#?}", lexed.collect::<Vec<SpannedToken>>());

    let parsed = Parser::new(Lexer::new(&contents));
    println!("{:#?} {:#?}", parsed.ast, parsed.ast_errors);
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