use core::fmt;
use guarded::guarded_unwrap;
use logos::{Lexer as LogosLexer, Logos, SpannedIter};

#[derive(Default, Debug, Clone, PartialEq)]
pub enum LexicalError {
    #[default]
    InvalidToken,

    Ignore
}

pub type Spanned<Token, Loc, Error> = Result<(Loc, Token, Loc), Error>;

#[derive(Logos, Clone, Debug, PartialEq)]
#[logos(skip r"[ \t\n\f]+", skip r"#.*\n?", error = LexicalError)]
pub enum Token {
    // Do not change the order of the operators.
    #[token("^")]
    OpPow,
    #[token("/")]
    OpDiv,
    #[token("//")]
    OpFloorDiv,
    #[token("%", priority = 5)]
    OpMod,
    #[token("*")]
    OpMult,
    #[token("+")]
    OpAdd,
    #[token("-")]
    OpSub,

    #[regex(r"\-\-\[=*\[", priority = 99, callback = |lex| multiline_string_block_callback(lex, 2))]
    CommentMulti(Result<usize, usize>),

    #[regex(r"\[=*\[", priority = 98, callback = |lex| multiline_string_block_callback(lex, 0))]
    StringMulti(Result<usize, usize>),

    #[regex(r"\-\-[^\[\n\f\r]*", priority = 98)]
    CommentSingle,

    #[token("{", priority = 1)]
    ScopeOpen,

    #[token("}", priority = 1)]
    ScopeClose,

    #[token("(", priority = 1)]
    ParensOpen,

    #[token(")", priority = 1)]
    ParensClose,

    #[token(",", priority = 1)]
    Comma,

    #[token(";", priority = 1)]
    SemiColon,

    #[token(":", priority = 1)]
    Colon,

    #[token(".", priority = 1)]
    Dot,

    #[token("=", priority = 1)]
    Equals,

    #[regex(r"[_A-Za-z][_A-Za-z\d]*|[_A-Za-z]+(-[A-Za-z\d_]+)+")]
    Identifier,

    #[regex(r"\$[_A-Za-z][_A-Za-z\d]*|[_A-Za-z]+(-[A-Za-z\d_]+)+", priority = 1)]
    TokenIdentifier,

    #[regex(r"\$![_A-Za-z][_A-Za-z\d]*|[_A-Za-z]+(-[A-Za-z\d_]+)+", priority = 1)]
    StaticTokenIdentifier,

    #[regex(r"&[_A-Za-z][_A-Za-z\d]*|[_A-Za-z]+(-[A-Za-z\d_]+)+", priority = 1)]
    StaticArgumentIdentifier,

    #[regex(r"([_A-Za-z][_A-Za-z\d]*|[_A-Za-z]+(-[A-Za-z\d_]+)+)!")]
    MacroIdentifier,

    #[regex(r"\.[_A-Za-z][_A-Za-z\d]*|[_A-Za-z]+(-[A-Za-z\d_]+)+", priority = 1)]
    TagSelectorOrEnumPart,

    #[regex(r"#[_A-Za-z][_A-Za-z\d]*|[_A-Za-z]+(-[A-Za-z\d_]+)+", priority = 1)]
    NameSelector,

    #[regex(r"::[_A-Za-z][_A-Za-z\d]*|[_A-Za-z]+(-[A-Za-z\d_]+)+", priority = 1)]
    PsuedoSelector,

    #[regex(r":[_A-Za-z][_A-Za-z\d]*|[_A-Za-z]+(-[A-Za-z\d_]+)+", priority = 1)]
    StateSelectorOrEnumPart,

    #[token(">", priority = 1)]
    ChildrenSelector,

    #[token(">>", priority = 1)]
    DescendantsSelector,

    #[token("@priority", priority = 1)]
    PriorityDeclaration,

    #[token("@derive", priority = 1)]
    DeriveDeclaration,

    #[token("@name", priority = 1)]
    NameDeclaration,

    #[token("@macro")]
    MacroDeclaration,

    #[token("@util")]
    UtilDeclaration,

    #[token("true")]
    BoolTrue,

    #[token("false")]
    BoolFalse,

    #[token("nil")]
    Nil,

    #[token("Enum")]
    EnumKeyword,

    #[regex(r"(?i)tw:[a-z]+(:\d+)?")]
    ColorTailwind,

    #[regex(r"(?i)skin:[a-z]+(:\d+)?")]
    ColorSkin,

    #[regex(r"(?i)bc:[a-z]+")]
    ColorBrick,

    #[regex(r"(?i)css:[a-z]+")]
    ColorCss,

    #[regex(r"#[\da-fA-F]+")]
    ColorHex,

    #[regex(r"[\d_]*\.?[\d_]+", priority = 4)]
    Number,

    #[regex(r"[\d_]*\.?[\d_]+%", priority = 45)]
    NumberScale,

    #[regex(r"[\d_]*\.?[\d_]+px", priority = 45)]
    NumberOffset,

    #[regex(r#""[^\"\n\t]*""#)]
    #[regex(r#"'[^\'\n\t]*'"#)]
    StringSingle,

    #[regex(r"rbxassetid://\d*")]
    #[regex(r"(rbxasset|rbxthumb|rbxgameasset|rbxhttp|rbxtemp|https?)://[^) ]*")]
    RbxAsset,

    #[regex(r"contentid://\d*", priority = 999)]
    RbxContent,

    Error,
    Expr(Expr)
}

#[derive(Logos, Debug, PartialEq, Clone)]
#[logos(skip r"[ \t\n\f]+", skip r"#.*\n?", error = LexicalError)]
enum MultilineStringToken {
    #[regex(r"\]=*\]")]
    ExitMultilineString,
}

fn multiline_string_block_callback(lex: &mut LogosLexer<Token>, sub_amount: usize) -> Result<usize, usize> {
    let mut multiline_comment_lexer = lex.clone().morph::<MultilineStringToken>();

    let start_token_len = multiline_comment_lexer.slice().len() - sub_amount;

    while let Some(token) = multiline_comment_lexer.next() {
        match token {
            Ok(MultilineStringToken::ExitMultilineString) => {
                if start_token_len == multiline_comment_lexer.slice().len() {
                    *lex = multiline_comment_lexer.morph();

                    return Ok(start_token_len - 2);
                }
            },
            _ => {},
        }
    }

    *lex = multiline_comment_lexer.morph();
    Err(start_token_len - 2)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    MacroDefinition,
    PriorityDefinition,
    PropertyAssignment,
    ScopeRuleDeclaration((Vec<Vec<Token>>, Vec<Token>)),
    Operation((Box<Token>, Box<Token>, Box<Token>)),
    Tuple((Option<Box<Token>>, Box<Vec<Token>>)),
    MacroCall((Box<Token>, Box<Vec<Vec<Token>>>)),
    Enum((Box<Token>, Box<Token>)),
    EnumShorthand(Box<Token>),
    AssetUrl(Box<Token>),
    ContentUrl(Box<Token>)
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

pub struct Lexer<'input> {
    token_stream: SpannedIter<'input, Token>
}

impl<'input> Lexer<'input> {
    pub fn new(input: &'input str) -> Self {
        Self { token_stream: Token::lexer(input).spanned() }
    }
}

impl<'input> Iterator for Lexer<'input> {
    type Item = Spanned<Token, usize, LexicalError>;

    fn next(&mut self) -> Option<Self::Item> {

        loop {
            let (token, span) = guarded_unwrap!(self.token_stream.next(), return None);
        
            match token {
                Ok(token) => match token {
                    // Ignores all single-line comments as well as multi-line
                    // comments with a valid opening and closing tag.
                    Token::CommentMulti(Ok(_)) |
                    Token::CommentSingle => continue,

                    _ => return Some(Ok((span.start, token, span.end)))
                },
                Err(_) => return Some(Ok((span.start, Token::Error, span.end))),
            }
        }
    }
}