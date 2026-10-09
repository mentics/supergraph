use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectAst {
    pub root: String,
    pub files: Vec<FileAst>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileAst {
    pub path: String,
    pub imports: Vec<ImportAst>,
    pub assignments: Vec<AssignmentAst>,
    pub calls: Vec<CallAst>,
    pub symbols: Vec<SymbolAst>,
    pub statements: Vec<StatementAst>,
    pub expressions: Vec<ExpressionAst>,
    pub conditions: Vec<ConditionAst>,
    pub definitions: Vec<DefinitionAst>,
    pub uses: Vec<UseAst>,
    pub returns: Vec<ReturnAst>,
    pub raises: Vec<RaiseAst>,
    pub field_accesses: Vec<FieldAccessAst>,
    pub index_accesses: Vec<IndexAccessAst>,
    /// Regions Tree-sitter could not parse; the rest of the file is still extracted.
    #[serde(default)]
    pub parse_errors: Vec<ParseErrorAst>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParseErrorKind {
    /// Source text the grammar could not match.
    Unparseable,
    /// A token the grammar expected but the source omitted.
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParseErrorAst {
    pub kind: ParseErrorKind,
    pub message: String,
    pub text: String,
    pub source_span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportAst {
    pub text: String,
    pub module: Option<String>,
    pub names: Vec<ImportNameAst>,
    pub source_span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportNameAst {
    pub name: String,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignmentAst {
    pub target: String,
    pub value: Option<String>,
    pub text: String,
    pub source_span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolAst {
    pub id: String,
    pub name: String,
    pub kind: SymbolKind,
    pub module_path: String,
    pub parent: Option<String>,
    pub parameters: Vec<ParamAst>,
    pub decorators: Vec<DecoratorAst>,
    pub return_type: Option<String>,
    pub body_span: Option<SourceSpan>,
    pub assignments: Vec<AssignmentAst>,
    pub calls: Vec<CallAst>,
    pub raises: Vec<RaiseAst>,
    pub statements: Vec<StatementAst>,
    pub expressions: Vec<ExpressionAst>,
    pub conditions: Vec<ConditionAst>,
    pub definitions: Vec<DefinitionAst>,
    pub uses: Vec<UseAst>,
    pub returns: Vec<ReturnAst>,
    pub field_accesses: Vec<FieldAccessAst>,
    pub index_accesses: Vec<IndexAccessAst>,
    pub source_span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
    Class,
    Function,
    Method,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecoratorAst {
    pub text: String,
    pub source_span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParamAst {
    pub name: String,
    pub text: String,
    pub source_span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallAst {
    pub callee: String,
    pub receiver: Option<String>,
    pub argument_names: Vec<String>,
    pub args_count: usize,
    pub context: CallContext,
    pub source_span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallContext {
    Body,
    ModuleInitializer,
    Decorator,
    Comprehension,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RaiseAst {
    pub text: String,
    pub value: Option<String>,
    pub owner_id: String,
    pub source_span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatementAst {
    pub kind: StatementKind,
    pub text: String,
    pub owner_id: String,
    pub source_span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatementKind {
    Assignment,
    Declaration,
    Expression,
    If,
    Loop,
    Return,
    Raise,
    Throw,
    Function,
    Class,
    Break,
    Continue,
    Try,
    Catch,
    Finally,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpressionAst {
    pub kind: ExpressionKind,
    pub text: String,
    pub owner_id: String,
    pub source_span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExpressionKind {
    Identifier,
    Literal,
    Call,
    FieldAccess,
    IndexAccess,
    Assignment,
    BinaryOperator,
    UnaryOperator,
    Conditional,
    Await,
    Yield,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionAst {
    pub kind: ConditionKind,
    pub text: String,
    pub owner_id: String,
    pub source_span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConditionKind {
    If,
    ElseIf,
    While,
    For,
    ConditionalExpression,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionAst {
    pub name: String,
    pub kind: DefinitionKind,
    pub text: String,
    pub owner_id: String,
    pub source_span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DefinitionKind {
    Class,
    Function,
    Parameter,
    Variable,
    Field,
    Type,
    Import,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UseAst {
    pub name: String,
    pub kind: UseKind,
    pub owner_id: String,
    pub source_span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UseKind {
    Identifier,
    Callee,
    Field,
    Type,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReturnAst {
    pub value: Option<String>,
    pub owner_id: String,
    pub source_span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldAccessAst {
    pub object: Option<String>,
    pub field: String,
    pub text: String,
    pub owner_id: String,
    pub source_span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexAccessAst {
    pub object: Option<String>,
    pub index: Option<String>,
    pub text: String,
    pub owner_id: String,
    pub source_span: SourceSpan,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceSpan {
    pub start_byte: u32,
    pub end_byte: u32,
    pub start_row: u32,
    pub start_column: u32,
    pub end_row: u32,
    pub end_column: u32,
}
