

use guarded::guarded_unwrap;
use phf_macros::phf_set;
use ropey::Rope;
use tokio::sync::{Mutex};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, InitializeParams, InitializeResult, Position, Range, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, Url};
use tower_lsp::{Client, LanguageServer, LspService, Server};

mod lexer;

use lalrpop_util::{lalrpop_mod, ParseError};
lalrpop_mod!(grammar);
use crate::lexer::{Lexer, LexicalError, Token};
use crate::grammar::RsmlParser;

struct Backend {
    client: Client,
    parser: Mutex<RsmlParser>,
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _params: InitializeParams) -> tower_lsp::jsonrpc::Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
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
}

struct Error<'a> {
    kind: String,
    range: Range,
    expected: Option<&'a Vec<String>>
}

impl<'a> Error<'a> {
    fn new(kind: &str, source_code: &str, location: (usize, usize), expected: Option<&'a Vec<String>>) -> Self {
        Self {
            kind: kind.to_string(),
            range: Range::from_offset(source_code, location),
            expected
        }
    }
}

fn expected_to_msg(expected: &Vec<String>) -> Option<String> {
    if expected.len() == 1 { return Some(format!("Expected `{}`", expected[0].clip(1, 1))) }

    let mut iter = expected.iter();

    let mut current_item = guarded_unwrap!(iter.next(), return None)
        .clip(1, 1);

    let mut msg = format!("Expected `{current_item}`");

    current_item = guarded_unwrap!(iter.next(), return Some(msg))
        .clip(1, 1);

    loop {
        let next_item = iter.next();
        
        if let Some(next_item) = next_item {
            let msg_item = format!(", `{}`", current_item);
            current_item = next_item.clip(1, 1);
            msg += &msg_item;

        } else {
            let msg_item = format!(" or `{}`", current_item);
            msg += &msg_item;
            break
        };
    }

    return Some(msg)
}

impl<'a> From<Error<'a>> for Diagnostic {
    fn from(error: Error) -> Self {
        let msg = match error.expected {
            Some(expected) => match expected_to_msg(expected) {
                Some(expected_msg) => format!("{}: {}", error.kind, expected_msg),
                None => error.kind 
            },
            None => error.kind
        };

        Self {
            range: error.range,
            severity: Some(DiagnosticSeverity::ERROR),
            code: None,
            code_description: None,
            source: Some("rsml-lsp".to_string()),
            message:msg,
            related_information: None,
            tags: None,
            data: None,
        }
    }
}

// Changes the location of a token to end of the
// previous token, instead of the current token.
fn unrecognised_token_to_missing_token_location(start_char_idx: usize, text: &str) -> (usize, usize) {
    let char_offset = {
        let mut char_offset = 0;
        let mut iter = text.chars().rev().skip(text.len() - start_char_idx);

        if let Some(next) = iter.next() {
            if next.is_whitespace() {
                char_offset += 1;

                for char in iter {
                    char_offset += 1;

                    if !char.is_whitespace() { break }
                }
            }
        }

        char_offset
    };

    let location = start_char_idx - char_offset;

    (location, location + 1)
}

static MISSING_TOKENS: phf::Set<&str> = phf_set! { "\",\"", "\";\"", "\"{\"", "\"}\"" };

fn should_convert_to_missing_token(expected: &Vec<String>) -> bool {
    let mut matches_count = 0;

    for item in expected {
        if MISSING_TOKENS.contains(&item) { matches_count += 1 }
    }

    matches_count == expected.len()
}

fn optional_token_matches(token: Option<&(usize, Token, usize)>, match_against: Token) -> bool {
    let token = guarded_unwrap!(token, return false);
    matches!(&token.1, match_against)
}

fn parse_error_to_diagnostic(
    err: &ParseError<usize, Token, LexicalError>,
    dropped_tokens: Option<&Vec<(usize, Token, usize)>>,
    source_code: &str
) -> Option<Diagnostic> {
    match err {
        ParseError::InvalidToken { location } =>
            Some(Error::new("Invalid Token", source_code, (*location, *location), None).into()),

        ParseError::UnrecognizedEof { location, expected } => 
            Some(Error::new("Unrecognized Eof", source_code, (*location, *location), Some(&expected)).into()),

        ParseError::UnrecognizedToken { token: (start, token, end), expected } => {
            // hacky fix to prevent macro calls from erroring. This fix may cause false positives but properly
            // handling macro calls will require a custom hand written parser.
            if matches!(token, Token::MacroIdentifier) {
                if let Some(dropped_tokens) = dropped_tokens {
                    if optional_token_matches(dropped_tokens.get(1), Token::ParensOpen) && {
                        let dropped_tokens_max_idx = dropped_tokens.len() - 1;

                        matches!(dropped_tokens[dropped_tokens_max_idx].1, Token::ParensClose) ||
                        optional_token_matches(dropped_tokens.get(dropped_tokens_max_idx - 1), Token::ParensClose)
                    } {
                        return None
                    }
                        
                } else {
                    return None
                }
            }

            if should_convert_to_missing_token(expected) {
                // Maps UnrecognizedToken error to MissingToken error.
                Some(Error::new(
                    "Missing Token", source_code,
                    unrecognised_token_to_missing_token_location(*start, source_code),
                    Some(&expected)
                ).into())

            } else {
                Some(Error::new("Unrecognized Token", source_code, (*start, *end), Some(&expected)).into())
            }
           
        },
        ParseError::ExtraToken { token: (start, _, end) } => 
            Some(Error::new("Extra Token", source_code, (*start, *end), None).into()),

        ParseError::User { error } => match error {
            LexicalError::InvalidToken => Some(Error::new("Invalid Token", source_code, (0, 0), None).into()),
            _ => None
        }
    }.into()
}

impl Backend {
    async fn parse_and_log(&self, source_code: &str, uri: Url) {
        let diagnostics = if source_code.len() == 0 {
            vec![]

        } else {
            let parser = self.parser.lock().await;

            let mut errors = vec![];
            let parsed = parser.parse(&mut errors, Lexer::new(source_code));

            //self.client.show_message(tower_lsp::lsp_types::MessageType::INFO, format!("{:#?}", errors)).await;

            let mut diagnostics = errors.iter()
                .filter_map(
                    |err| parse_error_to_diagnostic(
                        &err.error, Some(&err.dropped_tokens), source_code
                    )
                )
                .collect::<Vec<Diagnostic>>();

            if
                let Err(err) = parsed &&
                let Some(diagnostic) = parse_error_to_diagnostic(&err, None, source_code) {
                diagnostics.push(diagnostic);
            }

            diagnostics
        };

        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;
    }
}

trait RangeFromOffset {
    fn from_offset(text: &str, location: (usize, usize)) -> Range;
}

impl RangeFromOffset for Range {
    fn from_offset(text: &str, location: (usize, usize))  -> Range {
        let (start_char_idx, end_char_idx) = location;

        let rope = Rope::from_str(text);

        let start_line_idx = rope.char_to_line(start_char_idx);
        let start_line = rope.line_to_char(start_line_idx);
        let start_col = start_char_idx - start_line;

        let end_line_idx = rope.char_to_line(end_char_idx);
        let end_line = rope.line_to_char(end_line_idx);
        let end_col = end_char_idx - end_line;

        Range {
            start: Position {
                line: start_line_idx as u32,
                character: start_col as u32,
            },
            end: Position {
                line: end_line_idx as u32,
                character: end_col as u32,
            },
        }
    }
}

mod string_clip {
    pub trait StringClip {
        fn clip<'a>(&'a self, start: usize, end: usize) -> &'a str;
    }
    
    impl StringClip for str {
        fn clip<'a>(&'a self, start: usize, end: usize) -> &'a str {
            &self[start..self.len() - end]
        }
    }

    impl StringClip for String {
        fn clip<'a>(&'a self, start: usize, end: usize) -> &'a str {
            &self[start..self.len() - end]
        }
    }
}
use crate::string_clip::StringClip;

#[tokio::main]
async fn main() {
    let parser = RsmlParser::new();

    let (stdin, stdout) = (tokio::io::stdin(), tokio::io::stdout());

    let (service, socket) = LspService::new(|client| Backend {
        client,
        parser: Mutex::new(parser)
    });

    Server::new(stdin, stdout, socket).serve(service).await;
}


fn test() {
    let parser = RsmlParser::new();

    // The included .rsml file below is git-ignored, so you may need to create one yourself.
    let lexer = Lexer::new(include_str!("../test.rsml"));
    let mut errors = vec![];
    let parsed = parser.parse(&mut errors, lexer);

    //println!("{:#?}", lexer.collect::<Vec<Result<(usize, lexer::Token, usize), LexicalError>>>());
    println!("{:#?} {:#?}", parsed, errors);
}


