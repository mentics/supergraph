use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectAst {
    pub root: String,
    pub files: Vec<FileAst>,
    /// Package and workspace manifests (`Cargo.toml`, `package.json`, `pyproject.toml`, ...) for
    /// the analyzed language's ecosystem, found under the root and in enclosing directories up
    /// to the repository root.
    pub manifests: Vec<ManifestAst>,
}

/// A parsed package-manager manifest. Paths are relative to the analysis root, use `/`, and
/// may start with `..` for manifests in enclosing directories; `.` is the root itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestAst {
    pub path: String,
    pub dir: String,
    pub format: ManifestFormat,
    /// The distributable unit this manifest declares, if any.
    pub package: Option<PackageAst>,
    /// The workspace this manifest declares, if any.
    pub workspace: Option<WorkspaceAst>,
}

/// Package ecosystem of the language being analyzed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Ecosystem {
    Cargo,
    Npm,
    Python,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManifestFormat {
    CargoToml,
    PackageJson,
    PnpmWorkspaceYaml,
    PyprojectToml,
    SetupCfg,
    SetupPy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageAst {
    pub name: String,
    pub version: Option<String>,
    /// Compilation targets (Cargo only), each with its root source file.
    pub targets: Vec<TargetAst>,
    /// Source files the manifest names as entry points (npm `main`, `module`, `bin`).
    pub entry_files: Vec<String>,
    pub dependencies: Vec<DependencyAst>,
    /// Manifest path of the workspace this package is a member of (possibly its own manifest).
    pub workspace_manifest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceAst {
    /// Member patterns exactly as written, relative to the workspace directory.
    pub members: Vec<String>,
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetAst {
    pub name: String,
    pub kind: TargetKind,
    /// Root source file, relative to the analysis root.
    pub root_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TargetKind {
    Lib,
    Bin,
    Example,
    Test,
    Bench,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyAst {
    /// The depended-on package's own name (after any rename).
    pub name: String,
    /// Local name when the manifest renames the dependency.
    pub rename: Option<String>,
    /// Version requirement or specifier as written.
    pub requirement: Option<String>,
    pub kind: DependencyKind,
    pub optional: bool,
    /// Directory of a local path dependency, relative to the analysis root.
    pub path: Option<String>,
    /// Version is inherited from the enclosing workspace.
    pub workspace: bool,
    /// Optional-dependency or dependency group the entry was declared under.
    pub group: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DependencyKind {
    Normal,
    Dev,
    Build,
    Peer,
    Optional,
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
