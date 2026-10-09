use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{
    DefinitionAst, DefinitionKind as AstDefinitionKind, FieldAccessAst, IndexAccessAst, SourceSpan,
    UseAst,
};
use crate::supergraph::{
    self as sg, Confidence, ControlFlowNodeRole, DataFlowKind, DataFlowNodeRole, DiagnosticKind,
    EdgeFact, EdgeKind, ExpressionKind, NodeFact, NodeId, NodeKind, ProgramSupergraph, Severity,
    ValueKind, ValueRole, stable_id,
};

use super::{
    SemanticCallable, SemanticContext,
    callable_index::CallableIndex,
    control_dependence::{CallableCfg, callable_cfg_indexed},
    edge_id, graph_edge, graph_node, inference_evidence, insert_edge, insert_node, node_owner,
    span_contains, span_key,
};

const SG071_PRECISION: &str = "sg071-cfg-aware-reaching-definition";
const SG072_MERGE_PRECISION: &str = "sg072-branch-merge-value";
const SG072_LOOP_PRECISION: &str = "sg072-loop-carried-value";
const SG073_FIELD_WRITE_PRECISION: &str = "sg073-field-write-value";
const SG073_FIELD_READ_PRECISION: &str = "sg073-field-write-to-read-summary";
const SG073_INDEX_WRITE_PRECISION: &str = "sg073-index-write-value";
const SG073_INDEX_READ_PRECISION: &str = "sg073-index-write-to-read-summary";
const SG073_ALIAS_PRECISION: &str = "sg073-possible-alias-summary";

pub(crate) fn emit(graph: &mut ProgramSupergraph, context: &SemanticContext<'_>) {
    let index = CallableIndex::build(graph);
    for semantic in context.semantic_callables() {
        emit_callable(graph, &index, context, &semantic);
    }
}

pub(crate) fn definition_node_id(callable_id: &str, definition: &DefinitionAst) -> NodeId {
    stable_id(
        "df-node",
        &[
            callable_id,
            "definition",
            &definition.name,
            &span_key(definition.source_span),
            &definition.text,
        ],
    )
}

pub(crate) fn use_node_id(callable_id: &str, use_fact: &UseAst) -> NodeId {
    stable_id(
        "df-node",
        &[
            callable_id,
            "use",
            &use_fact.name,
            &span_key(use_fact.source_span),
        ],
    )
}

fn emit_callable(
    graph: &mut ProgramSupergraph,
    index: &CallableIndex,
    context: &SemanticContext<'_>,
    semantic: &SemanticCallable<'_>,
) {
    let callable_id = semantic.callable().callable_id.as_str();
    let owner_id = semantic.owner_id();
    let definitions = semantic
        .definitions()
        .iter()
        .filter(|definition| definition.owner_id == owner_id)
        .collect::<Vec<_>>();
    let uses = semantic
        .uses()
        .iter()
        .filter(|use_fact| use_fact.owner_id == owner_id)
        .collect::<Vec<_>>();
    let returns = semantic
        .returns()
        .iter()
        .filter(|return_fact| return_fact.owner_id == owner_id)
        .collect::<Vec<_>>();

    for definition in &definitions {
        let node_id = definition_node_id(callable_id, definition);
        insert_node(
            graph,
            graph_node(
                node_id.clone(),
                NodeKind::DataFlow,
                node_owner(semantic),
                Some(definition.source_span),
                Confidence::Exact,
                inference_evidence("normalized definition fact"),
                NodeFact::DataFlow(sg::DataFlowNode {
                    data_flow_node_id: node_id.clone(),
                    callable_id: callable_id.to_string(),
                    role: DataFlowNodeRole::Definition,
                    name: Some(definition.name.clone()),
                    text: definition.text.clone(),
                    semantic_kind: Some(format!("{:?}", definition.kind)),
                }),
            ),
        );
        add_defines_edge(graph, semantic, definition, &node_id);
    }

    for use_fact in &uses {
        let node_id = use_node_id(callable_id, use_fact);
        insert_node(
            graph,
            graph_node(
                node_id.clone(),
                NodeKind::DataFlow,
                node_owner(semantic),
                Some(use_fact.source_span),
                Confidence::Exact,
                inference_evidence("normalized use fact"),
                NodeFact::DataFlow(sg::DataFlowNode {
                    data_flow_node_id: node_id.clone(),
                    callable_id: callable_id.to_string(),
                    role: DataFlowNodeRole::Use,
                    name: Some(use_fact.name.clone()),
                    text: use_fact.name.clone(),
                    semantic_kind: Some(format!("{:?}", use_fact.kind)),
                }),
            ),
        );
        add_uses_edge(graph, context, semantic, use_fact, &node_id);
    }

    emit_value_instance_flows(graph, index, semantic, &definitions, &uses, &returns);
}

fn add_defines_edge(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    definition: &DefinitionAst,
    definition_id: &str,
) {
    let source_id = containing_statement_id(semantic, definition.source_span)
        .unwrap_or_else(|| semantic.callable().callable_id.clone());
    insert_edge(
        graph,
        graph_edge(
            edge_id("defines", source_id, definition_id, &definition.name),
            EdgeKind::Defines,
            source_id,
            definition_id.to_string(),
            node_owner(semantic),
            Some(definition.source_span),
            Confidence::Exact,
            inference_evidence("definition is owned by nearest normalized statement or callable"),
            EdgeFact::Defines(sg::Defines {
                callable_id: semantic.callable().callable_id.clone(),
                definition_id: definition_id.to_string(),
                name: definition.name.clone(),
            }),
        ),
    );
}

fn add_uses_edge(
    graph: &mut ProgramSupergraph,
    context: &SemanticContext<'_>,
    semantic: &SemanticCallable<'_>,
    use_fact: &UseAst,
    use_id: &str,
) {
    let source_id = containing_statement_id(semantic, use_fact.source_span)
        .or_else(|| containing_call_site_id(context, semantic, use_fact.source_span))
        .unwrap_or_else(|| semantic.callable().callable_id.clone());
    insert_edge(
        graph,
        graph_edge(
            edge_id("uses", source_id, use_id, &use_fact.name),
            EdgeKind::Uses,
            source_id,
            use_id.to_string(),
            node_owner(semantic),
            Some(use_fact.source_span),
            Confidence::Exact,
            inference_evidence("use is owned by nearest normalized statement/call or callable"),
            EdgeFact::Uses(sg::Uses {
                callable_id: semantic.callable().callable_id.clone(),
                use_id: use_id.to_string(),
                name: use_fact.name.clone(),
            }),
        ),
    );
}

fn emit_value_instance_flows(
    graph: &mut ProgramSupergraph,
    index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
    definitions: &[&DefinitionAst],
    uses: &[&UseAst],
    returns: &[&crate::ast::ReturnAst],
) {
    let callable_id = semantic.callable().callable_id.as_str();
    let expressions = expressions_for_callable(graph, index, callable_id);
    let expression_by_id = expressions
        .iter()
        .map(|expression| (expression.expression_id.clone(), expression.clone()))
        .collect::<BTreeMap<_, _>>();

    for expression in &expressions {
        let Some(target_id) = expression.value_id.as_deref() else {
            continue;
        };
        if !index.has_value(target_id) {
            continue;
        }
        let (flow_kind, precision, confidence, skip_first_child) = match expression.kind {
            ExpressionKind::Assignment => (
                DataFlowKind::AssignmentValue,
                "sg070-direct-assignment-expression-value",
                Confidence::Exact,
                true,
            ),
            ExpressionKind::BinaryOperator
            | ExpressionKind::UnaryOperator
            | ExpressionKind::Conditional
            | ExpressionKind::Await
            | ExpressionKind::Yield => (
                DataFlowKind::ExpressionOperand,
                "sg070-direct-expression-operand-value",
                Confidence::Exact,
                false,
            ),
            ExpressionKind::Call => (
                DataFlowKind::CallArgument,
                "sg070-direct-call-argument-to-call-result-value",
                Confidence::Probable,
                true,
            ),
            ExpressionKind::FieldAccess => (
                DataFlowKind::FieldAccess,
                "sg070-direct-field-value-without-field-alias-model",
                Confidence::Unknown,
                false,
            ),
            ExpressionKind::IndexAccess => (
                DataFlowKind::IndexAccess,
                "sg070-direct-index-value-without-index-alias-model",
                Confidence::Unknown,
                false,
            ),
            _ => continue,
        };

        for (ordinal, child_id) in expression.child_expression_ids.iter().enumerate() {
            if skip_first_child && ordinal == 0 {
                continue;
            }
            let Some(child) = expression_by_id.get(child_id) else {
                continue;
            };
            let Some(source_id) = child.value_id.as_deref() else {
                continue;
            };
            if source_id == target_id || !index.has_value(source_id) {
                continue;
            }
            add_value_data_flow_edge(
                graph,
                semantic,
                source_id,
                target_id,
                expression_value_name(expression),
                flow_kind,
                index.expression_span(expression.expression_id),
                precision,
                confidence,
            );
        }
    }

    emit_field_index_alias_flows(graph, index, semantic, definitions);

    for reaching_use in cfg_aware_reaching_uses(graph, index, semantic, definitions, uses) {
        let flow_kind = if returns
            .iter()
            .any(|return_fact| span_contains(return_fact.source_span, reaching_use.use_span))
        {
            DataFlowKind::DefinitionToReturn
        } else {
            DataFlowKind::DefinitionToUse
        };
        add_value_data_flow_edge(
            graph,
            semantic,
            &reaching_use.definition_value_id,
            &reaching_use.use_value_id,
            &reaching_use.name,
            flow_kind,
            Some(reaching_use.use_span),
            SG071_PRECISION,
            Confidence::Exact,
        );
    }

    emit_cfg_merge_value_flows(graph, index, semantic, definitions, uses);

    for return_fact in returns {
        let Some(return_value_id) = role_value_for_expression(
            graph,
            index,
            callable_id,
            return_fact.source_span,
            ValueRole::ReturnValue,
        ) else {
            continue;
        };
        let Some(source_id) = expression_value_for_text_in_span(
            graph,
            index,
            callable_id,
            return_fact.value.as_deref(),
            return_fact.source_span,
        ) else {
            continue;
        };
        if source_id == return_value_id {
            continue;
        }
        add_value_data_flow_edge(
            graph,
            semantic,
            &source_id,
            &return_value_id,
            return_fact.value.as_deref().unwrap_or("return"),
            DataFlowKind::ReturnValue,
            Some(return_fact.source_span),
            "sg070-direct-return-expression-value",
            Confidence::Exact,
        );
    }

    emit_merge_placeholder_values(graph, index, semantic);
}

fn emit_field_index_alias_flows(
    graph: &mut ProgramSupergraph,
    index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
    definitions: &[&DefinitionAst],
) {
    let callable_id = semantic.callable().callable_id.as_str();
    let expressions = expressions_for_callable(graph, index, callable_id);
    let expression_by_id = expressions
        .iter()
        .map(|expression| (expression.expression_id.clone(), expression.clone()))
        .collect::<BTreeMap<_, _>>();
    let access_expressions = access_expressions_by_span(graph, index, callable_id);
    let assignment_targets = assignment_targets(index, &expressions, &expression_by_id);
    let mut records = Vec::new();

    for field in semantic
        .field_accesses()
        .iter()
        .filter(|field| field.owner_id == semantic.owner_id())
    {
        let Some(expression) = access_expressions.get(&field.source_span) else {
            continue;
        };
        let Some(value_id) = expression.value_id.clone() else {
            continue;
        };
        let is_write = assignment_targets.contains(&expression.expression_id)
            || definitions.iter().any(|definition| {
                definition.kind == AstDefinitionKind::Field
                    && definition.source_span == field.source_span
            });
        records.push(AccessRecord {
            kind: AccessRecordKind::Field,
            span: field.source_span,
            expression_id: expression.expression_id.clone(),
            value_id,
            key: AccessKey::field(field),
            member: Some(field.field.clone()),
            object: field.object.clone(),
            is_write,
        });
    }

    for index in semantic
        .index_accesses()
        .iter()
        .filter(|index| index.owner_id == semantic.owner_id())
    {
        let Some(expression) = access_expressions.get(&index.source_span) else {
            continue;
        };
        let Some(value_id) = expression.value_id.clone() else {
            continue;
        };
        records.push(AccessRecord {
            kind: AccessRecordKind::Index,
            span: index.source_span,
            expression_id: expression.expression_id.clone(),
            value_id,
            key: AccessKey::index(index),
            member: index.index.clone(),
            object: index.object.clone(),
            is_write: assignment_targets.contains(&expression.expression_id),
        });
    }

    emit_access_write_value_flows(graph, semantic, &records, &expression_by_id);
    emit_exact_access_read_summaries(graph, semantic, &records);
    emit_alias_uncertain_access_summaries(graph, semantic, &records);
    emit_dynamic_access_diagnostics(graph, semantic, &records);
}

fn emit_access_write_value_flows(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    records: &[AccessRecord],
    expression_by_id: &BTreeMap<NodeId, sg::Expression>,
) {
    for record in records.iter().filter(|record| record.is_write) {
        let Some(expression) = expression_by_id.get(&record.expression_id) else {
            continue;
        };
        let Some(parent) = expression
            .parent_expression_id
            .as_deref()
            .and_then(|parent_id| expression_by_id.get(parent_id))
            .filter(|parent| parent.kind == ExpressionKind::Assignment)
        else {
            continue;
        };
        for child_id in parent.child_expression_ids.iter().skip(1) {
            let Some(source_id) = expression_by_id
                .get(child_id)
                .and_then(|child| child.value_id.as_deref())
            else {
                continue;
            };
            if source_id == record.value_id {
                continue;
            }
            add_value_data_flow_edge(
                graph,
                semantic,
                source_id,
                &record.value_id,
                record.flow_name(),
                record.flow_kind(),
                Some(record.span),
                record.write_precision(),
                Confidence::Probable,
            );
        }
    }
}

fn emit_exact_access_read_summaries(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    records: &[AccessRecord],
) {
    for read in records.iter().filter(|record| !record.is_write) {
        let Some(read_key) = &read.key else {
            continue;
        };
        for write in records.iter().filter(|record| record.is_write) {
            if write.span.start_byte > read.span.start_byte || write.value_id == read.value_id {
                continue;
            }
            if write.key.as_ref() != Some(read_key) {
                continue;
            }
            add_value_data_flow_edge(
                graph,
                semantic,
                &write.value_id,
                &read.value_id,
                read.flow_name(),
                read.flow_kind(),
                Some(read.span),
                read.read_precision(),
                Confidence::Probable,
            );
        }
    }
}

fn emit_alias_uncertain_access_summaries(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    records: &[AccessRecord],
) {
    for read in records.iter().filter(|record| !record.is_write) {
        for write in records.iter().filter(|record| record.is_write) {
            if write.span.start_byte > read.span.start_byte
                || write.value_id == read.value_id
                || write.kind != read.kind
                || write.key == read.key
                || write.member.is_none()
                || write.member != read.member
                || write.object == read.object
            {
                continue;
            }
            add_value_data_flow_edge(
                graph,
                semantic,
                &write.value_id,
                &read.value_id,
                read.flow_name(),
                read.flow_kind(),
                Some(read.span),
                SG073_ALIAS_PRECISION,
                Confidence::Unknown,
            );
            add_alias_uncertainty_diagnostic(
                graph,
                semantic,
                Some(read.span),
                vec![write.expression_id.clone(), read.expression_id.clone()],
                &format!(
                    "{} access may alias another base with the same member/key",
                    read.label()
                ),
            );
        }
    }
}

fn emit_dynamic_access_diagnostics(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    records: &[AccessRecord],
) {
    for record in records.iter().filter(|record| record.key.is_none()) {
        add_alias_uncertainty_diagnostic(
            graph,
            semantic,
            Some(record.span),
            vec![record.expression_id.clone()],
            &format!(
                "{} access has dynamic or unsupported alias identity; summary flow is uncertain",
                record.label()
            ),
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AccessRecord {
    kind: AccessRecordKind,
    span: SourceSpan,
    expression_id: NodeId,
    value_id: NodeId,
    key: Option<AccessKey>,
    member: Option<String>,
    object: Option<String>,
    is_write: bool,
}

impl AccessRecord {
    fn flow_kind(&self) -> DataFlowKind {
        match self.kind {
            AccessRecordKind::Field => DataFlowKind::FieldAccess,
            AccessRecordKind::Index => DataFlowKind::IndexAccess,
        }
    }

    fn flow_name(&self) -> &str {
        match self.kind {
            AccessRecordKind::Field => self.member.as_deref().unwrap_or("field"),
            AccessRecordKind::Index => self.member.as_deref().unwrap_or("index"),
        }
    }

    fn write_precision(&self) -> &'static str {
        match self.kind {
            AccessRecordKind::Field => SG073_FIELD_WRITE_PRECISION,
            AccessRecordKind::Index => SG073_INDEX_WRITE_PRECISION,
        }
    }

    fn read_precision(&self) -> &'static str {
        match self.kind {
            AccessRecordKind::Field => SG073_FIELD_READ_PRECISION,
            AccessRecordKind::Index => SG073_INDEX_READ_PRECISION,
        }
    }

    fn label(&self) -> &'static str {
        match self.kind {
            AccessRecordKind::Field => "field/property",
            AccessRecordKind::Index => "index",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessRecordKind {
    Field,
    Index,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AccessKey {
    Field { object: String, field: String },
    Index { object: String, index: String },
}

impl AccessKey {
    fn field(field: &FieldAccessAst) -> Option<Self> {
        Some(Self::Field {
            object: normalized_alias_subject(field.object.as_deref()?)?,
            field: field.field.clone(),
        })
    }

    fn index(index: &IndexAccessAst) -> Option<Self> {
        Some(Self::Index {
            object: normalized_alias_subject(index.object.as_deref()?)?,
            index: normalized_alias_subject(index.index.as_deref()?)?,
        })
    }
}

fn normalized_alias_subject(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()
        && trimmed
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '.')))
    .then(|| trimmed.to_string())
}

fn access_expressions_by_span(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: &str,
) -> BTreeMap<SourceSpan, sg::Expression> {
    expressions_for_callable(graph, index, callable_id)
        .into_iter()
        .filter(|expression| {
            matches!(
                expression.kind,
                ExpressionKind::FieldAccess | ExpressionKind::IndexAccess
            )
        })
        .filter_map(|expression| {
            Some((
                index.expression_span(expression.expression_id)?,
                expression,
            ))
        })
        .collect()
}

fn assignment_targets(
    index: &CallableIndex,
    expressions: &[sg::Expression],
    expression_by_id: &BTreeMap<NodeId, sg::Expression>,
) -> BTreeSet<NodeId> {
    expressions
        .iter()
        .filter(|expression| expression.kind == ExpressionKind::Assignment)
        .filter_map(|expression| expression.child_expression_ids.first())
        .filter_map(|target_id| expression_by_id.get(target_id))
        .filter(|target| {
            matches!(
                target.kind,
                ExpressionKind::FieldAccess | ExpressionKind::IndexAccess
            )
        })
        .filter(|target| index.expression_span(target.expression_id).is_some())
        .map(|target| target.expression_id.clone())
        .collect()
}

fn add_alias_uncertainty_diagnostic(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    span: Option<SourceSpan>,
    related: Vec<NodeId>,
    message: &str,
) {
    let diagnostic_id = stable_id(
        "diagnostic",
        &[
            &semantic.callable().callable_id,
            "sg073-alias-uncertainty",
            &span.map(span_key).unwrap_or_default(),
            message,
        ],
    );
    insert_node(
        graph,
        graph_node(
            diagnostic_id.clone(),
            NodeKind::Diagnostic,
            node_owner(semantic),
            span,
            Confidence::Unknown,
            inference_evidence("SG-073 alias uncertainty diagnostic"),
            NodeFact::Diagnostic(sg::Diagnostic {
                diagnostic_id,
                kind: DiagnosticKind::AliasUncertainty,
                severity: Severity::Warning,
                message: message.to_string(),
                artifact_id: Some(semantic.artifact().artifact_id.clone()),
                span,
                related,
            }),
        ),
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DefinitionRecord {
    definition_id: NodeId,
    name: String,
    span: SourceSpan,
    cfg_node_id: NodeId,
    value_id: Option<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UseRecord {
    name: String,
    span: SourceSpan,
    cfg_node_id: NodeId,
    value_id: Option<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReachingUse {
    name: String,
    use_span: SourceSpan,
    definition_value_id: NodeId,
    use_value_id: NodeId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReachingDefinitionAnalysis {
    cfg: CallableCfg,
    definitions_by_id: BTreeMap<NodeId, DefinitionRecord>,
    use_records: Vec<UseRecord>,
    in_sets: BTreeMap<NodeId, BTreeSet<NodeId>>,
    out_sets: BTreeMap<NodeId, BTreeSet<NodeId>>,
}

fn cfg_aware_reaching_uses(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
    definitions: &[&DefinitionAst],
    uses: &[&UseAst],
) -> Vec<ReachingUse> {
    if let Some(analysis) = cfg_reaching_definition_analysis(graph, index, semantic, definitions, uses) {
        return reaching_uses_from_analysis(analysis);
    }

    nearest_prior_reaching_uses(index, semantic, definitions, uses)
}

fn cfg_reaching_definition_analysis(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
    definitions: &[&DefinitionAst],
    uses: &[&UseAst],
) -> Option<ReachingDefinitionAnalysis> {
    let callable_id = semantic.callable().callable_id.as_str();
    let cfg = callable_cfg_indexed(graph, index, callable_id)?;

    let definition_records = definitions
        .iter()
        .filter_map(|definition| {
            let cfg_node_id = if definition.kind == AstDefinitionKind::Parameter {
                cfg.entry_node_id.clone()
            } else {
                executable_cfg_node_for_span(graph, index, callable_id, definition.source_span)?
            };
            Some(DefinitionRecord {
                definition_id: definition_node_id(callable_id, definition),
                name: definition.name.clone(),
                span: definition.source_span,
                cfg_node_id,
                value_id: index.definition_value_id(callable_id, &definition.name, definition.source_span),
            })
        })
        .collect::<Vec<_>>();
    let definitions_by_id = definition_records
        .iter()
        .map(|definition| (definition.definition_id.clone(), definition.clone()))
        .collect::<BTreeMap<_, _>>();

    let use_records = uses
        .iter()
        .filter_map(|use_fact| {
            Some(UseRecord {
                name: use_fact.name.clone(),
                span: use_fact.source_span,
                cfg_node_id: executable_cfg_node_for_span(
                    graph,
                    index,
                    callable_id,
                    use_fact.source_span,
                )?,
                value_id: index.use_value_id(callable_id, &use_fact.name, use_fact.source_span),
            })
        })
        .collect::<Vec<_>>();

    let mut gen_by_node = BTreeMap::<NodeId, BTreeSet<NodeId>>::new();
    let mut kill_names_by_node = BTreeMap::<NodeId, BTreeSet<String>>::new();
    for definition in &definition_records {
        if !cfg.is_reachable(&definition.cfg_node_id) {
            continue;
        }
        gen_by_node
            .entry(definition.cfg_node_id.clone())
            .or_default()
            .insert(definition.definition_id.clone());
        kill_names_by_node
            .entry(definition.cfg_node_id.clone())
            .or_default()
            .insert(definition.name.clone());
    }

    let all_definition_ids_by_name = definition_records.iter().fold(
        BTreeMap::<String, BTreeSet<NodeId>>::new(),
        |mut by_name, definition| {
            by_name
                .entry(definition.name.clone())
                .or_default()
                .insert(definition.definition_id.clone());
            by_name
        },
    );

    let mut in_sets = cfg
        .reachable_node_ids
        .iter()
        .map(|node_id| (node_id.clone(), BTreeSet::<NodeId>::new()))
        .collect::<BTreeMap<_, _>>();
    let mut out_sets = in_sets.clone();
    let mut changed = true;
    while changed {
        changed = false;
        for node_id in &cfg.reachable_node_ids {
            let next_in = cfg
                .predecessors_of(node_id)
                .into_iter()
                .filter(|predecessor| cfg.is_reachable(predecessor))
                .filter_map(|predecessor| out_sets.get(&predecessor))
                .fold(BTreeSet::new(), |mut reaching, predecessor_out| {
                    reaching.extend(predecessor_out.iter().cloned());
                    reaching
                });

            let mut next_out = next_in.clone();
            if let Some(kill_names) = kill_names_by_node.get(node_id) {
                for name in kill_names {
                    if let Some(killed_definition_ids) = all_definition_ids_by_name.get(name) {
                        for definition_id in killed_definition_ids {
                            next_out.remove(definition_id);
                        }
                    }
                }
            }
            if let Some(generated) = gen_by_node.get(node_id) {
                next_out.extend(generated.iter().cloned());
            }

            if in_sets.get(node_id) != Some(&next_in) {
                in_sets.insert(node_id.clone(), next_in);
                changed = true;
            }
            if out_sets.get(node_id) != Some(&next_out) {
                out_sets.insert(node_id.clone(), next_out);
                changed = true;
            }
        }
    }

    Some(ReachingDefinitionAnalysis {
        cfg,
        definitions_by_id,
        use_records,
        in_sets,
        out_sets,
    })
}

fn reaching_uses_from_analysis(analysis: ReachingDefinitionAnalysis) -> Vec<ReachingUse> {
    analysis
        .use_records
        .iter()
        .filter(|use_record| analysis.cfg.is_reachable(&use_record.cfg_node_id))
        .filter_map(|use_record| {
            Some((
                use_record,
                use_record.value_id.clone()?,
                analysis.in_sets.get(&use_record.cfg_node_id)?,
            ))
        })
        .flat_map(|(use_record, use_value_id, reaching_ids)| {
            reaching_ids
                .iter()
                .filter_map(|definition_id| analysis.definitions_by_id.get(definition_id))
                .filter(move |definition| definition.name == use_record.name)
                .filter_map(move |definition| {
                    let definition_value_id = definition.value_id.clone()?;
                    if definition_value_id == use_value_id {
                        return None;
                    }
                    Some(ReachingUse {
                        name: use_record.name.clone(),
                        use_span: use_record.span,
                        definition_value_id,
                        use_value_id: use_value_id.clone(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn emit_cfg_merge_value_flows(
    graph: &mut ProgramSupergraph,
    index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
    definitions: &[&DefinitionAst],
    uses: &[&UseAst],
) {
    let Some(analysis) = cfg_reaching_definition_analysis(graph, index, semantic, definitions, uses)
    else {
        return;
    };

    let merge_values = emit_branch_merge_value_flows(graph, index, semantic, &analysis);
    emit_merge_value_use_flows(graph, semantic, &analysis, &merge_values);
    let loop_values = emit_loop_carried_value_flows(graph, index, semantic, &analysis);
    emit_loop_carried_use_flows(graph, semantic, &analysis, &loop_values);
}

fn emit_branch_merge_value_flows(
    graph: &mut ProgramSupergraph,
    index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
    analysis: &ReachingDefinitionAnalysis,
) -> BTreeMap<(NodeId, String), MergeValueRecord> {
    let callable_id = semantic.callable().callable_id.as_str();
    let cfg_nodes = cfg_nodes_by_id(graph, index, callable_id);
    let mut merge_values = BTreeMap::new();

    for cfg_node_id in &analysis.cfg.reachable_node_ids {
        let Some((cfg_node, span)) = cfg_nodes.get(cfg_node_id) else {
            continue;
        };
        if cfg_node.role != ControlFlowNodeRole::Merge
            || analysis.cfg.predecessors_of(cfg_node_id).len() < 2
        {
            continue;
        }
        let reaching_by_name = reaching_value_definitions_by_name(
            analysis.in_sets.get(cfg_node_id).into_iter().flatten(),
            &analysis.definitions_by_id,
        );
        for (name, reaching_definitions) in reaching_by_name {
            if reaching_definitions.len() < 2 {
                continue;
            }
            let value_id = sg072_merge_value_id(callable_id, cfg_node_id, &name);
            insert_merge_value_node(
                graph,
                semantic,
                value_id.clone(),
                Some(name.clone()),
                *span,
                "SG-072 explicit branch merge value from multiple reaching definitions",
            );
            for definition in &reaching_definitions {
                add_value_data_flow_edge(
                    graph,
                    semantic,
                    &definition.value_id,
                    &value_id,
                    &name,
                    DataFlowKind::MergeValue,
                    *span,
                    SG072_MERGE_PRECISION,
                    Confidence::Unknown,
                );
            }
            merge_values.insert(
                (cfg_node_id.clone(), name.clone()),
                MergeValueRecord {
                    value_id,
                    name,
                    reaching_definition_ids: reaching_definitions
                        .iter()
                        .map(|definition| definition.definition_id.clone())
                        .collect(),
                },
            );
        }
    }

    merge_values
}

fn emit_merge_value_use_flows(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    analysis: &ReachingDefinitionAnalysis,
    merge_values: &BTreeMap<(NodeId, String), MergeValueRecord>,
) {
    for use_record in &analysis.use_records {
        let Some(use_value_id) = use_record.value_id.as_deref() else {
            continue;
        };
        let Some(use_reaching_ids) = analysis.in_sets.get(&use_record.cfg_node_id) else {
            continue;
        };
        for predecessor_id in analysis.cfg.predecessors_of(&use_record.cfg_node_id) {
            let Some(merge_record) =
                merge_values.get(&(predecessor_id.clone(), use_record.name.clone()))
            else {
                continue;
            };
            if merge_record
                .reaching_definition_ids
                .is_subset(use_reaching_ids)
            {
                add_value_data_flow_edge(
                    graph,
                    semantic,
                    &merge_record.value_id,
                    use_value_id,
                    &merge_record.name,
                    DataFlowKind::MergeValue,
                    Some(use_record.span),
                    SG072_MERGE_PRECISION,
                    Confidence::Unknown,
                );
            }
        }
    }
}

fn emit_loop_carried_value_flows(
    graph: &mut ProgramSupergraph,
    index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
    analysis: &ReachingDefinitionAnalysis,
) -> BTreeMap<(NodeId, String), LoopCarriedValueRecord> {
    let callable_id = semantic.callable().callable_id.as_str();
    let cfg_nodes = cfg_nodes_by_id(graph, index, callable_id);
    let loop_back_predecessors = loop_back_predecessors_by_target(graph, index, callable_id);
    let mut loop_values = BTreeMap::new();

    for (condition_id, backedge_predecessors) in loop_back_predecessors {
        if !analysis.cfg.is_reachable(&condition_id) {
            continue;
        }
        let Some((cfg_node, span)) = cfg_nodes.get(&condition_id) else {
            continue;
        };
        if cfg_node.role != ControlFlowNodeRole::Condition {
            continue;
        }

        let all_reaching = reaching_value_definitions_by_name(
            analysis.in_sets.get(&condition_id).into_iter().flatten(),
            &analysis.definitions_by_id,
        );
        let mut backedge_ids = BTreeSet::new();
        for predecessor_id in backedge_predecessors {
            if let Some(predecessor_out) = analysis.out_sets.get(&predecessor_id) {
                backedge_ids.extend(predecessor_out.iter().cloned());
            }
        }
        let backedge_reaching =
            reaching_value_definitions_by_name(backedge_ids.iter(), &analysis.definitions_by_id);

        for (name, all_definitions) in all_reaching {
            let Some(backedge_definitions) = backedge_reaching.get(&name) else {
                continue;
            };
            if backedge_definitions.is_empty() || all_definitions.len() < 2 {
                continue;
            }
            let value_id = sg072_loop_value_id(callable_id, &condition_id, &name);
            insert_merge_value_node(
                graph,
                semantic,
                value_id.clone(),
                Some(format!("loop-carried:{name}")),
                *span,
                "SG-072 explicit loop-carried value from loop backedge reaching definitions",
            );
            for definition in &all_definitions {
                add_value_data_flow_edge(
                    graph,
                    semantic,
                    &definition.value_id,
                    &value_id,
                    &name,
                    DataFlowKind::LoopCarriedValue,
                    *span,
                    SG072_LOOP_PRECISION,
                    Confidence::Unknown,
                );
            }
            loop_values.insert(
                (condition_id.clone(), name.clone()),
                LoopCarriedValueRecord {
                    value_id,
                    name,
                    reaching_definition_ids: all_definitions
                        .iter()
                        .map(|definition| definition.definition_id.clone())
                        .collect(),
                    backedge_definition_ids: backedge_definitions
                        .iter()
                        .map(|definition| definition.definition_id.clone())
                        .collect(),
                },
            );
        }
    }

    loop_values
}

fn emit_loop_carried_use_flows(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    analysis: &ReachingDefinitionAnalysis,
    loop_values: &BTreeMap<(NodeId, String), LoopCarriedValueRecord>,
) {
    for use_record in &analysis.use_records {
        let Some(use_value_id) = use_record.value_id.as_deref() else {
            continue;
        };
        let Some(use_reaching_ids) = analysis.in_sets.get(&use_record.cfg_node_id) else {
            continue;
        };
        for loop_record in loop_values.values() {
            if loop_record.name != use_record.name {
                continue;
            }
            if !loop_record
                .backedge_definition_ids
                .is_disjoint(use_reaching_ids)
                && !loop_record
                    .reaching_definition_ids
                    .is_disjoint(use_reaching_ids)
            {
                add_value_data_flow_edge(
                    graph,
                    semantic,
                    &loop_record.value_id,
                    use_value_id,
                    &loop_record.name,
                    DataFlowKind::LoopCarriedValue,
                    Some(use_record.span),
                    SG072_LOOP_PRECISION,
                    Confidence::Unknown,
                );
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReachingValueDefinition {
    definition_id: NodeId,
    value_id: NodeId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MergeValueRecord {
    value_id: NodeId,
    name: String,
    reaching_definition_ids: BTreeSet<NodeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LoopCarriedValueRecord {
    value_id: NodeId,
    name: String,
    reaching_definition_ids: BTreeSet<NodeId>,
    backedge_definition_ids: BTreeSet<NodeId>,
}

fn reaching_value_definitions_by_name<'a>(
    definition_ids: impl Iterator<Item = &'a NodeId>,
    definitions_by_id: &'a BTreeMap<NodeId, DefinitionRecord>,
) -> BTreeMap<String, Vec<ReachingValueDefinition>> {
    let mut by_name = BTreeMap::<String, BTreeMap<NodeId, ReachingValueDefinition>>::new();
    for definition_id in definition_ids {
        let Some(definition) = definitions_by_id.get(definition_id) else {
            continue;
        };
        let Some(value_id) = definition.value_id.clone() else {
            continue;
        };
        by_name.entry(definition.name.clone()).or_default().insert(
            definition.definition_id.clone(),
            ReachingValueDefinition {
                definition_id: definition.definition_id.clone(),
                value_id,
            },
        );
    }
    by_name
        .into_iter()
        .map(|(name, definitions)| (name, definitions.into_values().collect()))
        .collect()
}

fn insert_merge_value_node(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    value_id: NodeId,
    name: Option<String>,
    span: Option<SourceSpan>,
    evidence: &str,
) {
    insert_node(
        graph,
        graph_node(
            value_id.clone(),
            NodeKind::Value,
            node_owner(semantic),
            span,
            Confidence::Unknown,
            inference_evidence(evidence),
            NodeFact::Value(sg::Value {
                value_id,
                callable_id: Some(semantic.callable().callable_id.clone()),
                kind: ValueKind::Merge,
                role: ValueRole::Unknown,
                symbol_id: None,
                expression_id: None,
                call_site_id: None,
                name,
                ordinal: None,
                state_of_value_id: None,
                type_hint: None,
                literal: None,
            }),
        ),
    );
}

fn cfg_nodes_by_id(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: &str,
) -> BTreeMap<NodeId, (sg::ControlFlowNode, Option<SourceSpan>)> {
    index
        .nodes(graph, callable_id)
        .filter_map(|node| match &node.fact {
            NodeFact::ControlFlow(control) => {
                Some((control.cfg_node_id.clone(), (control.clone(), node.span)))
            }
            _ => None,
        })
        .collect()
}

fn loop_back_predecessors_by_target(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: &str,
) -> BTreeMap<NodeId, BTreeSet<NodeId>> {
    let mut predecessors = BTreeMap::<NodeId, BTreeSet<NodeId>>::new();
    for edge in index.control_flow_edges(graph, callable_id) {
        let EdgeFact::ControlFlow(flow) = &edge.fact else {
            continue;
        };
        if flow.callable_id == callable_id
            && (flow.flow_kind == sg::ControlFlowKind::LoopBack
                || matches!(
                    flow.outcome,
                    sg::ControlFlowOutcome::LoopBack | sg::ControlFlowOutcome::Continue
                ))
        {
            if let Some(target_id) = &edge.target_id {
                predecessors
                    .entry(target_id.clone())
                    .or_default()
                    .insert(edge.source_id.clone());
            }
        }
    }
    predecessors
}

fn sg072_merge_value_id(callable_id: &str, cfg_node_id: &str, name: &str) -> NodeId {
    stable_id("value", &[callable_id, "sg072-merge", cfg_node_id, name])
}

fn sg072_loop_value_id(callable_id: &str, cfg_node_id: &str, name: &str) -> NodeId {
    stable_id(
        "value",
        &[callable_id, "sg072-loop-carried", cfg_node_id, name],
    )
}

fn nearest_prior_reaching_uses(
    index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
    definitions: &[&DefinitionAst],
    uses: &[&UseAst],
) -> Vec<ReachingUse> {
    let callable_id = semantic.callable().callable_id.as_str();
    let definitions_by_name = definitions.iter().fold(
        BTreeMap::<String, Vec<&DefinitionAst>>::new(),
        |mut by_name, definition| {
            by_name
                .entry(definition.name.clone())
                .or_default()
                .push(*definition);
            by_name
        },
    );
    uses.iter()
        .filter_map(|use_fact| {
            let definition = nearest_prior_definition(
                definitions_by_name.get(&use_fact.name).map(Vec::as_slice),
                use_fact.source_span,
            )?;
            let definition_value_id = index.definition_value_id(callable_id, &definition.name, definition.source_span)?;
            let use_value_id = index.use_value_id(callable_id, &use_fact.name, use_fact.source_span)?;
            if definition_value_id == use_value_id {
                return None;
            }
            Some(ReachingUse {
                name: use_fact.name.clone(),
                use_span: use_fact.source_span,
                definition_value_id,
                use_value_id,
            })
        })
        .collect()
}

fn emit_merge_placeholder_values(
    graph: &mut ProgramSupergraph,
    index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
) {
    let callable_id = semantic.callable().callable_id.as_str();
    let cfg_nodes = index
        .nodes(graph, callable_id)
        .filter_map(|node| match &node.fact {
            NodeFact::ControlFlow(cfg) => {
                Some((cfg.clone(), node.owner.clone(), node.span))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let statements = index
        .nodes(graph, callable_id)
        .filter_map(|node| match &node.fact {
            NodeFact::Statement(statement) if statement.kind == sg::StatementKind::Loop =>
            {
                Some((statement.clone(), node.owner.clone(), node.span))
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    for (cfg, owner, span) in cfg_nodes {
        if cfg.role != ControlFlowNodeRole::Merge {
            continue;
        }
        let value_id = stable_id(
            "value",
            &[callable_id, "sg070-merge-placeholder", &cfg.cfg_node_id],
        );
        insert_node(
            graph,
            graph_node(
                value_id,
                NodeKind::Value,
                owner,
                span,
                Confidence::Unknown,
                inference_evidence(
                    "SG-070 uncertain merge value placeholder; CFG-aware reaching definitions are SG-071/SG-072",
                ),
                NodeFact::Value(sg::Value {
                    value_id: stable_id(
                        "value",
                        &[callable_id, "sg070-merge-placeholder", &cfg.cfg_node_id],
                    ),
                    callable_id: Some(callable_id.to_string()),
                    kind: ValueKind::Merge,
                    role: ValueRole::Unknown,
                    symbol_id: None,
                    expression_id: None,
                    call_site_id: None,
                    name: Some(cfg.semantic_kind.unwrap_or(cfg.label)),
                    ordinal: None,
                    state_of_value_id: None,
                    type_hint: None,
                    literal: None,
                }),
            ),
        );
    }

    for (statement, owner, span) in statements {
        let value_id = stable_id(
            "value",
            &[
                callable_id,
                "sg070-loop-carried-placeholder",
                &statement.statement_id,
            ],
        );
        insert_node(
            graph,
            graph_node(
                value_id.clone(),
                NodeKind::Value,
                owner,
                span,
                Confidence::Unknown,
                inference_evidence(
                    "SG-070 uncertain loop-carried value placeholder; loop reaching definitions are SG-071/SG-072",
                ),
                NodeFact::Value(sg::Value {
                    value_id,
                    callable_id: Some(callable_id.to_string()),
                    kind: ValueKind::Merge,
                    role: ValueRole::Unknown,
                    symbol_id: None,
                    expression_id: None,
                    call_site_id: None,
                    name: Some("loop-carried".to_string()),
                    ordinal: Some(statement.ordinal),
                    state_of_value_id: None,
                    type_hint: None,
                    literal: None,
                }),
            ),
        );
    }
}

fn add_value_data_flow_edge(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    source_id: &str,
    target_id: &str,
    name: &str,
    flow_kind: DataFlowKind,
    span: Option<SourceSpan>,
    precision: &str,
    confidence: Confidence,
) {
    insert_edge(
        graph,
        graph_edge(
            edge_id(
                "data-flow",
                source_id,
                target_id,
                &format!("{name}:{flow_kind:?}:{precision}"),
            ),
            EdgeKind::DataFlow,
            source_id.to_string(),
            target_id.to_string(),
            node_owner(semantic),
            span,
            confidence,
            inference_evidence(precision),
            EdgeFact::DataFlow(sg::DataFlow {
                callable_id: semantic.callable().callable_id.clone(),
                name: name.to_string(),
                flow_kind,
                precision: precision.to_string(),
            }),
        ),
    );
}

fn expressions_for_callable(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: &str,
) -> Vec<sg::Expression> {
    let mut expressions = index
        .nodes(graph, callable_id)
        .filter_map(|node| match &node.fact {
            NodeFact::Expression(expression) => Some(expression.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    expressions.sort_by_key(|expression| {
        (
            index
                .expression_span(expression.expression_id)
                .map(|span| span.start_byte)
                .unwrap_or(usize::MAX),
            expression.ordinal,
            expression.expression_id.clone(),
        )
    });
    expressions
}

fn expression_value_name(expression: &sg::Expression) -> &str {
    expression
        .normalized
        .identifier
        .as_deref()
        .or(expression.normalized.member.as_deref())
        .or(expression.original_text.as_deref())
        .unwrap_or("value")
}

fn role_value_for_expression(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: &str,
    enclosing_span: SourceSpan,
    role: ValueRole,
) -> Option<NodeId> {
    index
        .nodes(graph, callable_id)
        .find_map(|node| match &node.fact {
            NodeFact::Value(value)
                if value.role == role
                    && node
                        .span
                        .is_some_and(|span| span_contains(enclosing_span, span)) =>
            {
                Some(value.value_id.clone())
            }
            _ => None,
        })
}

fn expression_value_for_text_in_span(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: &str,
    text: Option<&str>,
    enclosing_span: SourceSpan,
) -> Option<NodeId> {
    index
        .nodes(graph, callable_id)
        .filter_map(|node| match &node.fact {
            NodeFact::Expression(expression) => Some((node, expression)),
            _ => None,
        })
        .filter(|(node, expression)| {
            node.span
                .is_some_and(|span| span_contains(enclosing_span, span))
                && text.is_none_or(|text| expression.original_text.as_deref() == Some(text))
        })
        .min_by_key(|(node, _)| {
            node.span
                .map(|span| span.end_byte.saturating_sub(span.start_byte))
                .unwrap_or(usize::MAX)
        })
        .and_then(|(_, expression)| expression.value_id.clone())
}

fn nearest_prior_definition<'a>(
    definitions: Option<&'a [&'a DefinitionAst]>,
    use_span: SourceSpan,
) -> Option<&'a DefinitionAst> {
    definitions?
        .iter()
        .copied()
        .filter(|definition| definition.source_span.start_byte <= use_span.start_byte)
        .max_by_key(|definition| {
            (
                definition.source_span.start_byte,
                definition.source_span.end_byte,
            )
        })
}

fn executable_cfg_node_for_span(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: &str,
    span: SourceSpan,
) -> Option<NodeId> {
    index
        .nodes(graph, callable_id)
        .filter_map(|node| match &node.fact {
            NodeFact::ControlFlow(control)
                if matches!(
                        control.role,
                        ControlFlowNodeRole::Condition
                            | ControlFlowNodeRole::Return
                            | ControlFlowNodeRole::Raise
                            | ControlFlowNodeRole::Statement
                    )
                    && node
                        .span
                        .is_some_and(|cfg_span| span_contains(cfg_span, span)) =>
            {
                let cfg_span = node.span?;
                let role_priority = match control.role {
                    ControlFlowNodeRole::Condition
                    | ControlFlowNodeRole::Return
                    | ControlFlowNodeRole::Raise => 0,
                    ControlFlowNodeRole::Statement => 1,
                    ControlFlowNodeRole::Entry
                    | ControlFlowNodeRole::Exit
                    | ControlFlowNodeRole::Merge => 2,
                };
                Some((
                    control.cfg_node_id.clone(),
                    cfg_span.end_byte.saturating_sub(cfg_span.start_byte),
                    role_priority,
                    cfg_span.start_byte,
                    cfg_span.end_byte,
                ))
            }
            _ => None,
        })
        .min_by_key(|(_, length, role_priority, start, end)| {
            (*length, *role_priority, *start, *end)
        })
        .map(|(node_id, _, _, _, _)| node_id)
}

fn containing_statement_id(semantic: &SemanticCallable<'_>, span: SourceSpan) -> Option<NodeId> {
    semantic
        .statements()
        .iter()
        .filter(|statement| statement.owner_id == semantic.owner_id())
        .filter(|statement| span_contains(statement.source_span, span))
        .min_by_key(|statement| {
            statement
                .source_span
                .end_byte
                .saturating_sub(statement.source_span.start_byte)
        })
        .map(|statement| super::cfg::statement_node_id(&semantic.callable().callable_id, statement))
}

fn containing_call_site_id(
    context: &SemanticContext<'_>,
    semantic: &SemanticCallable<'_>,
    span: SourceSpan,
) -> Option<NodeId> {
    semantic
        .calls()
        .iter()
        .filter(|call| span_contains(call.source_span, span))
        .find_map(|call| {
            context
                .call_site_for(semantic.callable().callable_id, call)
                .map(|call_site| call_site.call_site_id.clone())
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::analysis::source_graph::{build_python_supergraph, build_typescript_supergraph};
    use crate::ast::{
        CallAst, CallContext, ConditionAst, ConditionKind, DefinitionAst,
        DefinitionKind as AstDefinitionKind, ExpressionAst, ExpressionKind as AstExpressionKind,
        FieldAccessAst, FileAst, IndexAccessAst, ParamAst, ProjectAst, ReturnAst, SourceSpan,
        StatementAst, StatementKind as AstStatementKind, SymbolAst, SymbolKind, UseAst, UseKind,
    };
    use crate::supergraph::{
        ControlFlowNodeRole, DataFlowKind, DiagnosticKind, EdgeFact, NodeFact, NodeKind,
        ProgramDependenceGraphView, ProgramSupergraph, Uncertainty, ValueKind, ValueLiteral,
        ValueRole,
    };

    #[test]
    fn sg070_python_value_instance_data_flow_connects_values() {
        let graph = build_python_supergraph(&project("sample.py"));

        assert_sg070_value_flow(&graph);
    }

    #[test]
    fn sg070_typescript_value_instance_data_flow_connects_values() {
        let graph = build_typescript_supergraph(&project("sample.ts"));

        assert_sg070_value_flow(&graph);
    }

    #[test]
    fn sg071_python_reaching_definitions_follow_cfg_paths() {
        let graph = build_python_supergraph(&sg071_project("sample.py", true));

        assert_sg071_reaching_definitions(&graph);
    }

    #[test]
    fn sg071_typescript_reaching_definitions_follow_cfg_paths() {
        let graph = build_typescript_supergraph(&sg071_project("sample.ts", false));

        assert_sg071_reaching_definitions(&graph);
    }

    #[test]
    fn sg072_python_emits_branch_merge_and_loop_carried_values() {
        let graph = build_python_supergraph(&sg071_project("sample.py", true));

        assert_sg072_merge_and_loop_carried_values(&graph);
    }

    #[test]
    fn sg072_typescript_emits_branch_merge_and_loop_carried_values() {
        let graph = build_typescript_supergraph(&sg071_project("sample.ts", false));

        assert_sg072_merge_and_loop_carried_values(&graph);
    }

    #[test]
    fn sg073_python_models_field_index_and_alias_flows() {
        let graph = build_python_supergraph(&sg073_project("sample.py"));

        assert_sg073_field_index_and_alias_flows(&graph);
    }

    #[test]
    fn sg073_typescript_models_property_index_and_alias_flows() {
        let graph = build_typescript_supergraph(&sg073_project("sample.ts"));

        assert_sg073_field_index_and_alias_flows(&graph);
    }

    #[test]
    fn sg123_python_data_flow_and_slices_assert_precise_provenance() {
        assert_sg123_data_flow_and_slices(
            build_python_supergraph(&sg071_project("sample.py", true)),
            build_python_supergraph(&project("sample.py")),
            build_python_supergraph(&sg073_project("sample.py")),
        );
    }

    #[test]
    fn sg123_typescript_data_flow_and_slices_assert_precise_provenance() {
        assert_sg123_data_flow_and_slices(
            build_typescript_supergraph(&sg071_project("sample.ts", false)),
            build_typescript_supergraph(&project("sample.ts")),
            build_typescript_supergraph(&sg073_project("sample.ts")),
        );
    }

    fn assert_sg070_value_flow(graph: &ProgramSupergraph) {
        let sg070_edges = graph
            .edges
            .iter()
            .filter(|edge| {
                matches!(
                    &edge.fact,
                    EdgeFact::DataFlow(data_flow) if data_flow.precision.starts_with("sg070")
                )
            })
            .collect::<Vec<_>>();
        assert!(!sg070_edges.is_empty(), "missing SG-070 value flows");

        for edge in &sg070_edges {
            assert_eq!(node_kind(graph, &edge.source_id), Some(NodeKind::Value));
            assert_eq!(
                edge.target_id.as_ref().and_then(|id| node_kind(graph, id)),
                Some(NodeKind::Value),
                "SG-070 DataFlow target should be a value"
            );
        }

        for flow_kind in [
            DataFlowKind::ExpressionOperand,
            DataFlowKind::AssignmentValue,
            DataFlowKind::CallArgument,
            DataFlowKind::ReturnValue,
            DataFlowKind::FieldAccess,
            DataFlowKind::IndexAccess,
        ] {
            assert!(
                sg070_edges.iter().any(|edge| {
                    matches!(&edge.fact, EdgeFact::DataFlow(data_flow) if data_flow.flow_kind == flow_kind)
                }),
                "missing SG-070 value flow kind {flow_kind:?}"
            );
        }

        for value_kind in [
            ValueKind::Parameter,
            ValueKind::Literal,
            ValueKind::ComputedExpression,
            ValueKind::CallResult,
            ValueKind::Return,
            ValueKind::Field,
            ValueKind::Index,
            ValueKind::Merge,
        ] {
            assert!(
                graph.nodes.iter().any(|node| {
                    matches!(&node.fact, NodeFact::Value(value) if value.kind == value_kind)
                }),
                "missing value kind {value_kind:?}"
            );
        }

        assert!(
            graph.nodes.iter().any(|node| {
                matches!(
                    &node.fact,
                    NodeFact::Value(value)
                        if value.kind == ValueKind::Merge
                            && value.name.as_deref() == Some("loop-carried")
                            && node.uncertainty == crate::supergraph::Uncertainty::Possible
                )
            }),
            "loop-carried placeholder should be retained as an uncertain merge value"
        );
    }

    fn assert_sg071_reaching_definitions(graph: &ProgramSupergraph) {
        let edges = sg071_value_edges(graph);

        let branch_use = use_value_at(graph, span(101, 102));
        assert_reaches(graph, &edges, span(35, 40), &branch_use);
        assert_reaches(graph, &edges, span(65, 70), &branch_use);
        assert_not_reaches(graph, &edges, span(10, 15), &branch_use);

        let loop_rhs_use = use_value_at(graph, span(215, 216));
        assert_reaches(graph, &edges, span(35, 40), &loop_rhs_use);
        assert_reaches(graph, &edges, span(65, 70), &loop_rhs_use);
        assert_reaches(graph, &edges, span(211, 220), &loop_rhs_use);
        assert_not_reaches(graph, &edges, span(145, 151), &loop_rhs_use);
        assert_not_reaches(graph, &edges, span(175, 181), &loop_rhs_use);

        let after_loop_use = use_value_at(graph, span(236, 237));
        assert_reaches(graph, &edges, span(35, 40), &after_loop_use);
        assert_reaches(graph, &edges, span(65, 70), &after_loop_use);
        assert_reaches(graph, &edges, span(130, 135), &after_loop_use);
        assert_reaches(graph, &edges, span(160, 165), &after_loop_use);
        assert_reaches(graph, &edges, span(211, 220), &after_loop_use);
        assert_not_reaches(graph, &edges, span(145, 151), &after_loop_use);
        assert_not_reaches(graph, &edges, span(175, 181), &after_loop_use);

        let after_return_branch_use = use_value_at(graph, span(326, 327));
        assert_not_reaches(graph, &edges, span(275, 280), &after_return_branch_use);

        let return_use = use_value_at(graph, span(352, 353));
        assert_not_reaches(graph, &edges, span(365, 371), &return_use);

        assert!(
            graph.edges.iter().any(|edge| matches!(
                &edge.fact,
                EdgeFact::DataFlow(data_flow)
                    if data_flow.precision == super::SG071_PRECISION
                        && data_flow.flow_kind == DataFlowKind::DefinitionToReturn
            )),
            "SG-071 should classify reaching definitions in returns"
        );
        assert!(
            graph.edges.iter().all(|edge| match &edge.fact {
                EdgeFact::DataFlow(data_flow) if data_flow.precision == super::SG071_PRECISION => {
                    node_kind(graph, &edge.source_id) == Some(NodeKind::Value)
                        && edge.target_id.as_ref().and_then(|id| node_kind(graph, id))
                            == Some(NodeKind::Value)
                }
                _ => true,
            }),
            "SG-071 reaching definitions should use SG-070 value endpoints"
        );
    }

    fn assert_sg072_merge_and_loop_carried_values(graph: &ProgramSupergraph) {
        let merge_edges = sg072_value_edges(graph, DataFlowKind::MergeValue);
        let loop_edges = sg072_value_edges(graph, DataFlowKind::LoopCarriedValue);

        let branch_left = definition_value_at(graph, span(35, 40));
        let branch_right = definition_value_at(graph, span(65, 70));
        let branch_use = use_value_at(graph, span(101, 102));
        let branch_merge =
            merge_value_between_and_use(&merge_edges, &branch_left, &branch_right, &branch_use);
        assert_merge_value_node(graph, &branch_merge);

        let loop_update = definition_value_at(graph, span(211, 220));
        let loop_rhs_use = use_value_at(graph, span(215, 216));
        let after_loop_use = use_value_at(graph, span(236, 237));
        let loop_carried = loop_carried_value_reached_by(&loop_edges, &loop_update);
        assert!(
            loop_edges.contains(&(loop_carried.clone(), loop_rhs_use)),
            "loop update should feed later iterations through a loop-carried value"
        );
        assert!(
            loop_edges.contains(&(loop_carried, after_loop_use)),
            "loop-carried value should remain visible to loop exit uses"
        );

        assert!(
            graph.edges.iter().any(|edge| matches!(
                &edge.fact,
                EdgeFact::DataFlow(data_flow)
                    if data_flow.precision == super::SG072_MERGE_PRECISION
                        && data_flow.flow_kind == DataFlowKind::MergeValue
                        && edge.uncertainty == crate::supergraph::Uncertainty::Possible
            )),
            "SG-072 branch merge edges should carry explicit uncertainty"
        );
        assert!(
            graph.edges.iter().any(|edge| matches!(
                &edge.fact,
                EdgeFact::DataFlow(data_flow)
                    if data_flow.precision == super::SG072_LOOP_PRECISION
                        && data_flow.flow_kind == DataFlowKind::LoopCarriedValue
                        && edge.uncertainty == crate::supergraph::Uncertainty::Possible
            )),
            "SG-072 loop-carried edges should carry explicit uncertainty"
        );
        assert!(
            graph.nodes.iter().all(|node| {
                !matches!(
                    &node.fact,
                    NodeFact::Value(value)
                        if value.kind == ValueKind::Merge
                            && value.name.as_deref() == Some("y")
                )
            }),
            "single-definition variables should not get unnecessary merge values"
        );
    }

    fn assert_sg073_field_index_and_alias_flows(graph: &ProgramSupergraph) {
        let sg073_edges = graph
            .edges
            .iter()
            .filter(|edge| {
                matches!(
                    &edge.fact,
                    EdgeFact::DataFlow(data_flow) if data_flow.precision.starts_with("sg073")
                )
            })
            .collect::<Vec<_>>();
        assert!(!sg073_edges.is_empty(), "missing SG-073 value flows");

        for edge in &sg073_edges {
            assert_eq!(node_kind(graph, &edge.source_id), Some(NodeKind::Value));
            assert_eq!(
                edge.target_id.as_ref().and_then(|id| node_kind(graph, id)),
                Some(NodeKind::Value),
                "SG-073 DataFlow target should be a value"
            );
        }

        let seed_use = expression_value_at(graph, span(42, 46));
        let field_write = expression_value_at(graph, span(30, 39));
        let field_read = expression_value_at(graph, span(57, 66));
        let read_use = expression_value_at(graph, span(85, 89));
        let index_write = expression_value_at(graph, span(70, 82));
        let index_read = expression_value_at(graph, span(101, 113));
        let alias_read = expression_value_at(graph, span(133, 144));

        let edges = data_flow_edges(graph);
        assert!(
            edges.contains(&(seed_use.clone(), field_write.clone())),
            "field write should receive the assigned value"
        );
        assert!(
            edges.contains(&(field_write.clone(), field_read.clone())),
            "field read should summarize the prior same-field write"
        );
        assert!(
            edges.contains(&(read_use.clone(), index_write.clone())),
            "index write should receive the assigned value"
        );
        assert!(
            edges.contains(&(index_write.clone(), index_read.clone())),
            "index read should summarize the prior same-index write"
        );
        assert!(
            edges.contains(&(field_write.clone(), alias_read.clone())),
            "possible alias read should receive an uncertain same-member summary"
        );

        assert_reachable_forward(&edges, &seed_use, &field_read);
        assert_reachable_backward(&edges, &field_read, &seed_use);
        assert_reachable_forward(&edges, &read_use, &index_read);
        assert_reachable_backward(&edges, &index_read, &read_use);

        assert!(
            graph.nodes.iter().any(|node| matches!(
                &node.fact,
                NodeFact::Diagnostic(diagnostic)
                    if diagnostic.kind == DiagnosticKind::AliasUncertainty
                        && diagnostic.related.contains(&alias_expression_id(graph))
            )),
            "alias-sensitive field access should emit an AliasUncertainty diagnostic"
        );
        assert!(
            graph.edges.iter().any(|edge| matches!(
                &edge.fact,
                EdgeFact::DataFlow(data_flow)
                    if data_flow.precision == super::SG073_ALIAS_PRECISION
                        && edge.source_id == field_write
                        && edge.target_id.as_deref() == Some(alias_read.as_str())
            )),
            "possible alias summary should carry SG-073 alias precision"
        );
    }

    fn assert_sg123_data_flow_and_slices(
        branch_graph: ProgramSupergraph,
        expression_graph: ProgramSupergraph,
        alias_graph: ProgramSupergraph,
    ) {
        assert_sg123_branch_loop_merge_contracts(&branch_graph);
        assert_sg123_literal_operator_call_return_contracts(&expression_graph);
        assert_sg123_field_alias_and_slice_contracts(&alias_graph);
    }

    fn assert_sg123_branch_loop_merge_contracts(graph: &ProgramSupergraph) {
        let dfg = graph.data_flow_view();
        let pdg = ProgramDependenceGraphView::new(graph);

        let x_one = definition_value_at(graph, span(35, 40));
        let x_two = definition_value_at(graph, span(65, 70));
        let y_use = use_value_at(graph, span(101, 102));
        let x_update = definition_value_at(graph, span(211, 220));
        let loop_rhs = use_value_at(graph, span(215, 216));
        let after_loop_use = use_value_at(graph, span(236, 237));
        let x_three = definition_value_at(graph, span(275, 280));
        let return_use = use_value_at(graph, span(297, 298));
        let done_guard = definition_value_at(graph, span(21, 25));

        let merge_edges = sg072_value_edges(graph, DataFlowKind::MergeValue);
        let branch_merge = merge_value_between_and_use(&merge_edges, &x_one, &x_two, &y_use);
        assert_value_node(
            graph,
            &branch_merge,
            ValueKind::Merge,
            ValueRole::Unknown,
            Some("x"),
            Uncertainty::Possible,
        );
        assert_data_flow_edge(
            graph,
            &x_one,
            &branch_merge,
            DataFlowKind::MergeValue,
            super::SG072_MERGE_PRECISION,
            Uncertainty::Possible,
            Some(span(20, 90)),
        );
        assert_data_flow_edge(
            graph,
            &branch_merge,
            &y_use,
            DataFlowKind::MergeValue,
            super::SG072_MERGE_PRECISION,
            Uncertainty::Possible,
            Some(span(101, 102)),
        );

        let loop_edges = sg072_value_edges(graph, DataFlowKind::LoopCarriedValue);
        let loop_carried = loop_carried_value_reached_by(&loop_edges, &x_update);
        assert_value_node(
            graph,
            &loop_carried,
            ValueKind::Merge,
            ValueRole::Unknown,
            Some("loop-carried:x"),
            Uncertainty::Possible,
        );
        assert_data_flow_edge(
            graph,
            &x_update,
            &loop_carried,
            DataFlowKind::LoopCarriedValue,
            super::SG072_LOOP_PRECISION,
            Uncertainty::Possible,
            Some(span(116, 120)),
        );
        assert_data_flow_edge(
            graph,
            &loop_carried,
            &loop_rhs,
            DataFlowKind::LoopCarriedValue,
            super::SG072_LOOP_PRECISION,
            Uncertainty::Possible,
            Some(span(215, 216)),
        );
        assert_data_flow_edge(
            graph,
            &loop_carried,
            &after_loop_use,
            DataFlowKind::LoopCarriedValue,
            super::SG072_LOOP_PRECISION,
            Uncertainty::Possible,
            Some(span(236, 237)),
        );

        assert_data_flow_edge(
            graph,
            &x_three,
            &return_use,
            DataFlowKind::DefinitionToReturn,
            super::SG071_PRECISION,
            Uncertainty::Exact,
            Some(span(297, 298)),
        );
        assert!(
            !dfg.value_backward_slice(&return_use)
                .value_ids
                .contains(&definition_value_at(graph, span(365, 371))),
            "backward slice from return use must exclude unreachable later definitions"
        );

        let return_cfg = cfg_node_at_role(graph, span(290, 300), ControlFlowNodeRole::Return);
        let done_cfg = cfg_node_at_role(graph, span(263, 267), ControlFlowNodeRole::Condition);
        let return_slice = pdg.return_backward_slice(&return_cfg);
        assert!(return_slice.value_ids.contains(&x_three));
        assert!(return_slice.control_condition_ids.contains(&done_cfg));
        assert!(
            return_slice
                .path_conditions
                .iter()
                .any(|condition| condition.controlling_cfg_node_id == done_cfg),
            "return slice should preserve path-condition evidence for the done guard"
        );

        let done_condition = condition_node_at(graph, span(263, 267));
        let branch_slice = pdg.branch_backward_slice(&done_condition);
        assert!(branch_slice.value_ids.contains(&done_guard));
    }

    fn assert_sg123_literal_operator_call_return_contracts(graph: &ProgramSupergraph) {
        let dfg = graph.data_flow_view();
        let pdg = ProgramDependenceGraphView::new(graph);

        let items_operand = expression_value_at(graph, span(28, 33));
        let index_operand = expression_value_at(graph, span(34, 39));
        let index_read = expression_value_at(graph, span(28, 40));
        let literal_one = expression_value_at(graph, span(43, 44));
        let operator = expression_value_at(graph, span(28, 44));
        let total_def = definition_value_at(graph, span(20, 25));
        let total_arg = expression_value_at(graph, span(81, 86));
        let field_arg = expression_value_at(graph, span(87, 88));
        let call_expression = expression_value_at(graph, span(76, 88));
        let call_result = value_by_role_at(graph, span(70, 88), ValueRole::CallResult);
        let sent_def = definition_value_at(graph, span(69, 73));
        let sent_return_use = use_value_at(graph, span(138, 142));

        assert_value_node(
            graph,
            &literal_one,
            ValueKind::Literal,
            ValueRole::Unknown,
            None,
            Uncertainty::Exact,
        );
        assert_value_literal(graph, &literal_one, ValueLiteral::Integer("1".to_string()));
        assert_value_node(
            graph,
            &call_result,
            ValueKind::CallResult,
            ValueRole::CallResult,
            Some("send"),
            Uncertainty::Exact,
        );

        assert_data_flow_edge(
            graph,
            &items_operand,
            &index_read,
            DataFlowKind::IndexAccess,
            "sg070-direct-index-value-without-index-alias-model",
            Uncertainty::Possible,
            Some(span(28, 40)),
        );
        assert_data_flow_edge(
            graph,
            &index_operand,
            &index_read,
            DataFlowKind::IndexAccess,
            "sg070-direct-index-value-without-index-alias-model",
            Uncertainty::Possible,
            Some(span(28, 40)),
        );
        assert_data_flow_edge(
            graph,
            &index_read,
            &operator,
            DataFlowKind::ExpressionOperand,
            "sg070-direct-expression-operand-value",
            Uncertainty::Exact,
            Some(span(28, 44)),
        );
        assert_data_flow_edge(
            graph,
            &literal_one,
            &operator,
            DataFlowKind::ExpressionOperand,
            "sg070-direct-expression-operand-value",
            Uncertainty::Exact,
            Some(span(28, 44)),
        );
        assert_data_flow_edge(
            graph,
            &operator,
            &total_def,
            DataFlowKind::AssignmentValue,
            "sg070-direct-assignment-expression-value",
            Uncertainty::Exact,
            Some(span(20, 45)),
        );
        assert_data_flow_edge(
            graph,
            &total_arg,
            &call_expression,
            DataFlowKind::CallArgument,
            "sg070-direct-call-argument-to-call-result-value",
            Uncertainty::Probable,
            Some(span(76, 88)),
        );
        assert_data_flow_edge(
            graph,
            &field_arg,
            &call_expression,
            DataFlowKind::CallArgument,
            "sg070-direct-call-argument-to-call-result-value",
            Uncertainty::Probable,
            Some(span(76, 88)),
        );
        assert_data_flow_edge(
            graph,
            &call_expression,
            &sent_def,
            DataFlowKind::AssignmentValue,
            "sg070-direct-assignment-expression-value",
            Uncertainty::Exact,
            Some(span(69, 90)),
        );
        assert_data_flow_edge(
            graph,
            &sent_def,
            &sent_return_use,
            DataFlowKind::DefinitionToReturn,
            super::SG071_PRECISION,
            Uncertainty::Exact,
            Some(span(138, 142)),
        );

        let literal_forward = dfg.value_forward_slice(&literal_one);
        assert!(literal_forward.value_ids.contains(&operator));
        assert!(literal_forward.value_ids.contains(&total_def));

        let call_forward = dfg.value_forward_slice(&total_arg);
        assert!(call_forward.value_ids.contains(&call_expression));

        let return_cfg = cfg_node_at_role(graph, span(131, 142), ControlFlowNodeRole::Return);
        let return_slice = pdg.return_backward_slice(&return_cfg);
        assert!(return_slice.value_ids.contains(&call_expression));
        assert!(return_slice.value_ids.contains(&sent_def));

        let call_site = call_site_at(graph, span(70, 88));
        let call_slice = pdg.call_backward_slice(&call_site);
        assert!(call_slice.value_ids.contains(&total_arg));
        assert!(call_slice.value_ids.contains(&field_arg));
        assert!(call_slice.value_ids.contains(&call_expression));
    }

    fn assert_sg123_field_alias_and_slice_contracts(graph: &ProgramSupergraph) {
        let dfg = graph.data_flow_view();

        let seed = definition_value_at(graph, span(23, 27));
        let seed_use = expression_value_at(graph, span(42, 46));
        let field_write = expression_value_at(graph, span(30, 39));
        let field_read = expression_value_at(graph, span(57, 66));
        let read_def = definition_value_at(graph, span(50, 66));
        let read_use = expression_value_at(graph, span(85, 89));
        let index_write = expression_value_at(graph, span(70, 82));
        let index_read = expression_value_at(graph, span(101, 113));
        let alias_read = expression_value_at(graph, span(133, 144));

        assert_value_node(
            graph,
            &field_write,
            ValueKind::Field,
            ValueRole::Unknown,
            Some("value"),
            Uncertainty::Possible,
        );
        assert_value_node(
            graph,
            &index_write,
            ValueKind::Index,
            ValueRole::Unknown,
            Some("items[index]"),
            Uncertainty::Possible,
        );
        assert_data_flow_edge(
            graph,
            &seed_use,
            &field_write,
            DataFlowKind::FieldAccess,
            super::SG073_FIELD_WRITE_PRECISION,
            Uncertainty::Probable,
            Some(span(30, 39)),
        );
        assert_data_flow_edge(
            graph,
            &field_write,
            &field_read,
            DataFlowKind::FieldAccess,
            super::SG073_FIELD_READ_PRECISION,
            Uncertainty::Probable,
            Some(span(57, 66)),
        );
        assert_data_flow_edge(
            graph,
            &read_def,
            &read_use,
            DataFlowKind::DefinitionToUse,
            super::SG071_PRECISION,
            Uncertainty::Exact,
            Some(span(85, 89)),
        );
        assert_data_flow_edge(
            graph,
            &read_use,
            &index_write,
            DataFlowKind::IndexAccess,
            super::SG073_INDEX_WRITE_PRECISION,
            Uncertainty::Probable,
            Some(span(70, 82)),
        );
        assert_data_flow_edge(
            graph,
            &index_write,
            &index_read,
            DataFlowKind::IndexAccess,
            super::SG073_INDEX_READ_PRECISION,
            Uncertainty::Probable,
            Some(span(101, 113)),
        );
        assert_data_flow_edge(
            graph,
            &field_write,
            &alias_read,
            DataFlowKind::FieldAccess,
            super::SG073_ALIAS_PRECISION,
            Uncertainty::Possible,
            Some(span(133, 144)),
        );

        let forward = dfg.value_forward_slice(&seed);
        assert!(forward.value_ids.contains(&field_read));
        assert!(forward.value_ids.contains(&index_read));
        assert!(forward.value_ids.contains(&alias_read));
        assert!(
            forward
                .diagnostic_ids
                .iter()
                .any(|id| diagnostic_kind(graph, id) == Some(DiagnosticKind::AliasUncertainty)),
            "forward slice should surface alias uncertainty diagnostics"
        );

        let alias_backward = dfg.value_backward_slice(&alias_read);
        assert!(alias_backward.value_ids.contains(&seed));
        assert!(alias_backward.value_ids.contains(&field_write));
        assert!(
            alias_backward
                .diagnostic_ids
                .iter()
                .any(|id| diagnostic_kind(graph, id) == Some(DiagnosticKind::AliasUncertainty)),
            "backward slice should preserve alias uncertainty diagnostics"
        );
    }

    fn assert_data_flow_edge(
        graph: &ProgramSupergraph,
        source_id: &str,
        target_id: &str,
        flow_kind: DataFlowKind,
        precision: &str,
        uncertainty: Uncertainty,
        span: Option<SourceSpan>,
    ) {
        let edge = graph
            .edges
            .iter()
            .find(|edge| {
                edge.kind == crate::supergraph::EdgeKind::DataFlow
                    && edge.source_id == source_id
                    && edge.target_id.as_deref() == Some(target_id)
                    && matches!(
                        &edge.fact,
                        EdgeFact::DataFlow(data_flow)
                            if data_flow.flow_kind == flow_kind
                                && data_flow.precision == precision
                    )
            })
            .unwrap_or_else(|| {
                panic!(
                    "missing {flow_kind:?} DataFlow edge {source_id} -> {target_id} with precision {precision}"
                )
            });
        assert_eq!(
            edge.uncertainty, uncertainty,
            "unexpected uncertainty for {flow_kind:?} edge {source_id} -> {target_id} with precision {precision}"
        );
        assert_eq!(edge.span, span);
        assert!(
            edge.owner
                .callable_id
                .as_deref()
                .is_some_and(is_process_callable_id),
            "DataFlow edge should retain process callable ownership"
        );
        assert!(
            !edge.fact_id.is_empty() && !edge.payload_hash.is_empty(),
            "DataFlow edge should have refreshed fact identity"
        );
        assert!(
            edge.evidence
                .iter()
                .any(|evidence| !evidence.summary.is_empty()),
            "DataFlow edge should preserve analysis evidence for {precision}"
        );
        assert_eq!(
            node_kind(graph, &edge.source_id),
            Some(NodeKind::Value),
            "DataFlow source should be a value"
        );
        assert_eq!(
            edge.target_id.as_ref().and_then(|id| node_kind(graph, id)),
            Some(NodeKind::Value),
            "DataFlow target should be a value"
        );
    }

    fn assert_value_node(
        graph: &ProgramSupergraph,
        value_id: &str,
        kind: ValueKind,
        role: ValueRole,
        name: Option<&str>,
        uncertainty: Uncertainty,
    ) {
        let node = graph
            .nodes
            .iter()
            .find(|node| {
                matches!(
                    &node.fact,
                    NodeFact::Value(value) if value.value_id == value_id
                )
            })
            .unwrap_or_else(|| panic!("missing value node {value_id}"));
        let NodeFact::Value(value) = &node.fact else {
            unreachable!("value node matched above")
        };
        assert_eq!(value.kind, kind);
        assert_eq!(value.role, role);
        if let Some(name) = name {
            assert_eq!(value.name.as_deref(), Some(name));
        }
        assert!(
            value
                .callable_id
                .as_deref()
                .is_some_and(is_process_callable_id),
            "value payload should retain the process callable ID convention"
        );
        assert!(
            node.owner
                .callable_id
                .as_deref()
                .is_some_and(is_process_callable_id),
            "value node should retain process callable ownership"
        );
        assert_eq!(node.uncertainty, uncertainty);
        assert!(
            node.span.is_some(),
            "value node should preserve source span"
        );
        assert!(
            !node.fact_id.is_empty() && !node.payload_hash.is_empty(),
            "value node should have refreshed fact identity"
        );
        assert!(
            node.evidence
                .iter()
                .any(|evidence| !evidence.summary.is_empty()),
            "value node should preserve semantic evidence"
        );
    }

    fn assert_value_literal(
        graph: &ProgramSupergraph,
        value_id: &str,
        expected_literal: ValueLiteral,
    ) {
        let literal = graph.nodes.iter().find_map(|node| match &node.fact {
            NodeFact::Value(value) if value.value_id == value_id => value.literal.clone(),
            _ => None,
        });
        assert_eq!(literal, Some(expected_literal));
    }

    fn diagnostic_kind(graph: &ProgramSupergraph, diagnostic_id: &str) -> Option<DiagnosticKind> {
        graph.nodes.iter().find_map(|node| match &node.fact {
            NodeFact::Diagnostic(diagnostic) if diagnostic.diagnostic_id == diagnostic_id => {
                Some(diagnostic.kind)
            }
            _ => None,
        })
    }

    fn call_site_at(graph: &ProgramSupergraph, source_span: SourceSpan) -> String {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::CallSite(call_site) if node.span == Some(source_span) => {
                    Some(call_site.call_site_id.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing call site at {source_span:?}"))
    }

    fn value_by_role_at(
        graph: &ProgramSupergraph,
        source_span: SourceSpan,
        role: ValueRole,
    ) -> String {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Value(value) if node.span == Some(source_span) && value.role == role => {
                    Some(value.value_id.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing {role:?} value at {source_span:?}"))
    }

    fn cfg_node_at_role(
        graph: &ProgramSupergraph,
        source_span: SourceSpan,
        role: ControlFlowNodeRole,
    ) -> String {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::ControlFlow(control)
                    if node.span == Some(source_span) && control.role == role =>
                {
                    Some(control.cfg_node_id.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing {role:?} CFG node at {source_span:?}"))
    }

    fn condition_node_at(graph: &ProgramSupergraph, source_span: SourceSpan) -> String {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Condition(condition) if node.span == Some(source_span) => {
                    Some(condition.condition_id.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing condition at {source_span:?}"))
    }

    fn is_process_callable_id(callable_id: &str) -> bool {
        callable_id == "sample.process" || callable_id == "sample:process"
    }

    fn sg071_value_edges(graph: &ProgramSupergraph) -> BTreeSet<(String, String)> {
        graph
            .edges
            .iter()
            .filter_map(|edge| match &edge.fact {
                EdgeFact::DataFlow(data_flow) if data_flow.precision == super::SG071_PRECISION => {
                    Some((edge.source_id.clone(), edge.target_id.clone()?))
                }
                _ => None,
            })
            .collect()
    }

    fn data_flow_edges(graph: &ProgramSupergraph) -> BTreeSet<(String, String)> {
        graph
            .edges
            .iter()
            .filter_map(|edge| match &edge.fact {
                EdgeFact::DataFlow(_) => Some((edge.source_id.clone(), edge.target_id.clone()?)),
                _ => None,
            })
            .collect()
    }

    fn assert_reachable_forward(edges: &BTreeSet<(String, String)>, source: &str, target: &str) {
        assert!(
            reachable(edges, source, target, false),
            "expected {source} to reach {target} through forward DataFlow edges"
        );
    }

    fn assert_reachable_backward(edges: &BTreeSet<(String, String)>, source: &str, target: &str) {
        assert!(
            reachable(edges, source, target, true),
            "expected {source} to reach {target} through backward DataFlow edges"
        );
    }

    fn reachable(
        edges: &BTreeSet<(String, String)>,
        source: &str,
        target: &str,
        reverse: bool,
    ) -> bool {
        let mut seen = BTreeSet::new();
        let mut frontier = vec![source.to_string()];
        while let Some(current) = frontier.pop() {
            if current == target {
                return true;
            }
            if !seen.insert(current.clone()) {
                continue;
            }
            for (edge_source, edge_target) in edges {
                let next = if reverse && edge_target == &current {
                    Some(edge_source)
                } else if !reverse && edge_source == &current {
                    Some(edge_target)
                } else {
                    None
                };
                if let Some(next) = next {
                    frontier.push(next.clone());
                }
            }
        }
        false
    }

    fn sg072_value_edges(
        graph: &ProgramSupergraph,
        expected_kind: DataFlowKind,
    ) -> BTreeSet<(String, String)> {
        graph
            .edges
            .iter()
            .filter_map(|edge| match &edge.fact {
                EdgeFact::DataFlow(data_flow)
                    if data_flow.precision.starts_with("sg072")
                        && data_flow.flow_kind == expected_kind =>
                {
                    Some((edge.source_id.clone(), edge.target_id.clone()?))
                }
                _ => None,
            })
            .collect()
    }

    fn merge_value_between_and_use(
        edges: &BTreeSet<(String, String)>,
        left_definition: &str,
        right_definition: &str,
        use_value: &str,
    ) -> String {
        let left_targets = edges
            .iter()
            .filter_map(|(source, target)| (source == left_definition).then_some(target.clone()))
            .collect::<BTreeSet<_>>();
        let right_targets = edges
            .iter()
            .filter_map(|(source, target)| (source == right_definition).then_some(target.clone()))
            .collect::<BTreeSet<_>>();
        let merge_value = left_targets
            .intersection(&right_targets)
            .find(|merge_value| edges.contains(&(merge_value.to_string(), use_value.to_string())))
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "missing merge value shared by definitions {left_definition} and {right_definition} that reaches use {use_value}"
                )
            });
        merge_value
    }

    fn loop_carried_value_reached_by(
        edges: &BTreeSet<(String, String)>,
        definition: &str,
    ) -> String {
        edges
            .iter()
            .find_map(|(source, target)| (source == definition).then_some(target.clone()))
            .unwrap_or_else(|| panic!("missing loop-carried value reached by {definition}"))
    }

    fn assert_merge_value_node(graph: &ProgramSupergraph, value_id: &str) {
        assert!(
            graph.nodes.iter().any(|node| matches!(
                &node.fact,
                NodeFact::Value(value)
                    if value.value_id == value_id
                        && value.kind == ValueKind::Merge
                        && node.uncertainty == crate::supergraph::Uncertainty::Possible
            )),
            "expected {value_id} to be an uncertain merge value node"
        );
    }

    fn assert_reaches(
        graph: &ProgramSupergraph,
        edges: &BTreeSet<(String, String)>,
        definition_span: SourceSpan,
        use_value_id: &str,
    ) {
        let definition_value_id = definition_value_at(graph, definition_span);
        assert!(
            edges.contains(&(definition_value_id.clone(), use_value_id.to_string())),
            "expected definition at {definition_span:?} to reach use value {use_value_id}"
        );
    }

    fn assert_not_reaches(
        graph: &ProgramSupergraph,
        edges: &BTreeSet<(String, String)>,
        definition_span: SourceSpan,
        use_value_id: &str,
    ) {
        let definition_value_id = definition_value_at(graph, definition_span);
        assert!(
            !edges.contains(&(definition_value_id.clone(), use_value_id.to_string())),
            "did not expect definition at {definition_span:?} to reach use value {use_value_id}"
        );
    }

    fn definition_value_at(graph: &ProgramSupergraph, source_span: SourceSpan) -> String {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Definition(definition) if node.span == Some(source_span) => {
                    definition.value_id.clone()
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing definition value at {source_span:?}"))
    }

    fn use_value_at(graph: &ProgramSupergraph, source_span: SourceSpan) -> String {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Use(use_fact) if node.span == Some(source_span) => {
                    use_fact.value_id.clone()
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing use value at {source_span:?}"))
    }

    fn expression_value_at(graph: &ProgramSupergraph, source_span: SourceSpan) -> String {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Expression(expression) if node.span == Some(source_span) => {
                    expression.value_id.clone()
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing expression value at {source_span:?}"))
    }

    fn alias_expression_id(graph: &ProgramSupergraph) -> String {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Expression(expression) if node.span == Some(span(133, 144)) => {
                    Some(expression.expression_id.clone())
                }
                _ => None,
            })
            .expect("missing alias field expression")
    }

    fn node_kind(graph: &ProgramSupergraph, node_id: &str) -> Option<NodeKind> {
        graph
            .nodes
            .iter()
            .find(|node| node.node_id == node_id)
            .map(|node| node.kind)
    }

    fn project(path: &str) -> ProjectAst {
        ProjectAst {
            root: String::new(),
            files: vec![FileAst {
                path: path.to_string(),
                imports: Vec::new(),
                assignments: Vec::new(),
                calls: Vec::new(),
                symbols: vec![SymbolAst {
                    id: "sample:process".to_string(),
                    name: "process".to_string(),
                    kind: SymbolKind::Function,
                    module_path: "sample".to_string(),
                    parent: None,
                    parameters: vec![
                        param("items", 5, 10),
                        param("index", 12, 17),
                        param("obj", 19, 22),
                    ],
                    decorators: Vec::new(),
                    return_type: None,
                    body_span: Some(span(20, 150)),
                    assignments: Vec::new(),
                    calls: vec![CallAst {
                        callee: "send".to_string(),
                        receiver: None,
                        argument_names: Vec::new(),
                        args_count: 2,
                        context: CallContext::Body,
                        source_span: span(70, 88),
                    }],
                    raises: Vec::new(),
                    statements: vec![
                        statement(
                            AstStatementKind::Assignment,
                            "total = items[index] + 1",
                            20,
                            45,
                        ),
                        statement(AstStatementKind::Assignment, "field = obj.value", 46, 68),
                        statement(
                            AstStatementKind::Assignment,
                            "sent = send(total, field)",
                            69,
                            90,
                        ),
                        statement(AstStatementKind::Loop, "while total", 91, 130),
                        statement(AstStatementKind::Assignment, "total = total + 1", 100, 125),
                        statement(AstStatementKind::Return, "return sent", 131, 142),
                    ],
                    expressions: expressions(),
                    conditions: vec![ConditionAst {
                        kind: ConditionKind::While,
                        text: "total".to_string(),
                        owner_id: "sample:process".to_string(),
                        source_span: span(97, 102),
                    }],
                    definitions: vec![
                        definition("items", AstDefinitionKind::Parameter, "items", 5, 10),
                        definition("index", AstDefinitionKind::Parameter, "index", 12, 17),
                        definition("obj", AstDefinitionKind::Parameter, "obj", 19, 22),
                        definition("total", AstDefinitionKind::Variable, "total", 20, 25),
                        definition("field", AstDefinitionKind::Variable, "field", 46, 51),
                        definition("sent", AstDefinitionKind::Variable, "sent", 69, 73),
                        definition("total", AstDefinitionKind::Variable, "total", 100, 105),
                    ],
                    uses: vec![
                        use_fact("items", UseKind::Identifier, 28, 33),
                        use_fact("index", UseKind::Identifier, 34, 39),
                        use_fact("obj", UseKind::Identifier, 54, 57),
                        use_fact("total", UseKind::Identifier, 75, 80),
                        use_fact("field", UseKind::Identifier, 82, 87),
                        use_fact("total", UseKind::Identifier, 97, 102),
                        use_fact("total", UseKind::Identifier, 108, 113),
                        use_fact("sent", UseKind::Identifier, 138, 142),
                    ],
                    returns: vec![ReturnAst {
                        value: Some("sent".to_string()),
                        owner_id: "sample:process".to_string(),
                        source_span: span(131, 142),
                    }],
                    field_accesses: vec![FieldAccessAst {
                        object: Some("obj".to_string()),
                        field: "value".to_string(),
                        text: "obj.value".to_string(),
                        owner_id: "sample:process".to_string(),
                        source_span: span(54, 63),
                    }],
                    index_accesses: vec![IndexAccessAst {
                        object: Some("items".to_string()),
                        index: Some("index".to_string()),
                        text: "items[index]".to_string(),
                        owner_id: "sample:process".to_string(),
                        source_span: span(28, 40),
                    }],
                    source_span: span(0, 150),
                }],
                statements: Vec::new(),
                expressions: Vec::new(),
                conditions: Vec::new(),
                definitions: Vec::new(),
                uses: Vec::new(),
                returns: Vec::new(),
                raises: Vec::new(),
                field_accesses: Vec::new(),
                index_accesses: Vec::new(),
                    parse_errors: Vec::new(),
                }],
        }
    }

    fn sg073_project(path: &str) -> ProjectAst {
        ProjectAst {
            root: String::new(),
            files: vec![FileAst {
                path: path.to_string(),
                imports: Vec::new(),
                assignments: Vec::new(),
                calls: Vec::new(),
                symbols: vec![SymbolAst {
                    id: "sample:process".to_string(),
                    name: "process".to_string(),
                    kind: SymbolKind::Function,
                    module_path: "sample".to_string(),
                    parent: None,
                    parameters: vec![
                        param("obj", 1, 4),
                        param("alias", 5, 10),
                        param("items", 11, 16),
                        param("index", 17, 22),
                        param("seed", 23, 27),
                    ],
                    decorators: Vec::new(),
                    return_type: None,
                    body_span: Some(span(30, 160)),
                    assignments: Vec::new(),
                    calls: Vec::new(),
                    raises: Vec::new(),
                    statements: vec![
                        statement(AstStatementKind::Assignment, "obj.value = seed", 30, 46),
                        statement(AstStatementKind::Assignment, "read = obj.value", 50, 66),
                        statement(AstStatementKind::Assignment, "items[index] = read", 70, 89),
                        statement(AstStatementKind::Assignment, "out = items[index]", 95, 113),
                        statement(
                            AstStatementKind::Assignment,
                            "alias_read = alias.value",
                            120,
                            144,
                        ),
                    ],
                    expressions: sg073_expressions(),
                    conditions: Vec::new(),
                    definitions: vec![
                        definition("obj", AstDefinitionKind::Parameter, "obj", 1, 4),
                        definition("alias", AstDefinitionKind::Parameter, "alias", 5, 10),
                        definition("items", AstDefinitionKind::Parameter, "items", 11, 16),
                        definition("index", AstDefinitionKind::Parameter, "index", 17, 22),
                        definition("seed", AstDefinitionKind::Parameter, "seed", 23, 27),
                        definition("value", AstDefinitionKind::Field, "obj.value", 30, 39),
                        definition(
                            "read",
                            AstDefinitionKind::Variable,
                            "read = obj.value",
                            50,
                            66,
                        ),
                        definition(
                            "out",
                            AstDefinitionKind::Variable,
                            "out = items[index]",
                            95,
                            113,
                        ),
                        definition(
                            "alias_read",
                            AstDefinitionKind::Variable,
                            "alias_read = alias.value",
                            120,
                            144,
                        ),
                    ],
                    uses: vec![
                        use_fact("seed", UseKind::Identifier, 42, 46),
                        use_fact("obj", UseKind::Identifier, 57, 60),
                        use_fact("read", UseKind::Identifier, 85, 89),
                        use_fact("items", UseKind::Identifier, 101, 106),
                        use_fact("index", UseKind::Identifier, 107, 112),
                        use_fact("alias", UseKind::Identifier, 133, 138),
                    ],
                    returns: Vec::new(),
                    field_accesses: vec![
                        FieldAccessAst {
                            object: Some("obj".to_string()),
                            field: "value".to_string(),
                            text: "obj.value".to_string(),
                            owner_id: "sample:process".to_string(),
                            source_span: span(30, 39),
                        },
                        FieldAccessAst {
                            object: Some("obj".to_string()),
                            field: "value".to_string(),
                            text: "obj.value".to_string(),
                            owner_id: "sample:process".to_string(),
                            source_span: span(57, 66),
                        },
                        FieldAccessAst {
                            object: Some("alias".to_string()),
                            field: "value".to_string(),
                            text: "alias.value".to_string(),
                            owner_id: "sample:process".to_string(),
                            source_span: span(133, 144),
                        },
                    ],
                    index_accesses: vec![
                        IndexAccessAst {
                            object: Some("items".to_string()),
                            index: Some("index".to_string()),
                            text: "items[index]".to_string(),
                            owner_id: "sample:process".to_string(),
                            source_span: span(70, 82),
                        },
                        IndexAccessAst {
                            object: Some("items".to_string()),
                            index: Some("index".to_string()),
                            text: "items[index]".to_string(),
                            owner_id: "sample:process".to_string(),
                            source_span: span(101, 113),
                        },
                    ],
                    source_span: span(0, 160),
                }],
                statements: Vec::new(),
                expressions: Vec::new(),
                conditions: Vec::new(),
                definitions: Vec::new(),
                uses: Vec::new(),
                returns: Vec::new(),
                raises: Vec::new(),
                field_accesses: Vec::new(),
                index_accesses: Vec::new(),
                    parse_errors: Vec::new(),
                }],
        }
    }

    fn sg071_project(path: &str, python: bool) -> ProjectAst {
        let branch_text = if python {
            "if flag:\n    x = 1\nelse:\n    x = 2"
        } else {
            "if (flag) {\n  x = 1;\n} else {\n  x = 2;\n}"
        };
        let loop_text = if python {
            "while keep:\n    if stop:\n        x = 4\n        break\n        x = 44\n    if skip:\n        x = 5\n        continue\n        x = 55\n    x = x + 1"
        } else {
            "while (keep) {\n  if (stop) {\n    x = 4;\n    break;\n    x = 44;\n  }\n  if (skip) {\n    x = 5;\n    continue;\n    x = 55;\n  }\n  x = x + 1;\n}"
        };
        let stop_text = if python {
            "if stop:\n        x = 4\n        break\n        x = 44"
        } else {
            "if (stop) {\n    x = 4;\n    break;\n    x = 44;\n  }"
        };
        let skip_text = if python {
            "if skip:\n        x = 5\n        continue\n        x = 55"
        } else {
            "if (skip) {\n    x = 5;\n    continue;\n    x = 55;\n  }"
        };
        let done_text = if python {
            "if done:\n    x = 3\n    return x"
        } else {
            "if (done) {\n  x = 3;\n  return x;\n}"
        };
        ProjectAst {
            root: String::new(),
            files: vec![FileAst {
                path: path.to_string(),
                imports: Vec::new(),
                assignments: Vec::new(),
                calls: Vec::new(),
                symbols: vec![SymbolAst {
                    id: "sample:process".to_string(),
                    name: "process".to_string(),
                    kind: SymbolKind::Function,
                    module_path: "sample".to_string(),
                    parent: None,
                    parameters: vec![
                        param("flag", 1, 5),
                        param("keep", 6, 10),
                        param("stop", 11, 15),
                        param("skip", 16, 20),
                        param("done", 21, 25),
                    ],
                    decorators: Vec::new(),
                    return_type: None,
                    body_span: Some(span(1, 380)),
                    assignments: Vec::new(),
                    calls: Vec::new(),
                    raises: Vec::new(),
                    statements: vec![
                        statement(AstStatementKind::Assignment, "x = 0", 10, 15),
                        statement(AstStatementKind::If, branch_text, 20, 90),
                        statement(AstStatementKind::Assignment, "x = 1", 35, 40),
                        statement(AstStatementKind::Assignment, "x = 2", 65, 70),
                        statement(AstStatementKind::Assignment, "y = x", 95, 103),
                        statement(AstStatementKind::Loop, loop_text, 110, 225),
                        statement(AstStatementKind::If, stop_text, 125, 155),
                        statement(AstStatementKind::Assignment, "x = 4", 130, 135),
                        statement(AstStatementKind::Break, "break", 138, 143),
                        statement(AstStatementKind::Assignment, "x = 44", 145, 151),
                        statement(AstStatementKind::If, skip_text, 155, 185),
                        statement(AstStatementKind::Assignment, "x = 5", 160, 165),
                        statement(AstStatementKind::Continue, "continue", 168, 176),
                        statement(AstStatementKind::Assignment, "x = 55", 175, 181),
                        statement(AstStatementKind::Assignment, "x = x + 1", 211, 220),
                        statement(AstStatementKind::Assignment, "after_loop = x", 230, 238),
                        statement(AstStatementKind::If, done_text, 260, 310),
                        statement(AstStatementKind::Assignment, "x = 3", 275, 280),
                        statement(AstStatementKind::Return, "return x", 290, 300),
                        statement(AstStatementKind::Assignment, "after_done = x", 320, 328),
                        statement(AstStatementKind::Return, "return x", 345, 355),
                        statement(AstStatementKind::Assignment, "x = 99", 365, 371),
                    ],
                    expressions: sg071_expressions(),
                    conditions: vec![
                        condition(ConditionKind::If, "flag", 23, 27),
                        condition(ConditionKind::While, "keep", 116, 120),
                        condition(ConditionKind::If, "stop", 128, 132),
                        condition(ConditionKind::If, "skip", 158, 162),
                        condition(ConditionKind::If, "done", 263, 267),
                    ],
                    definitions: vec![
                        definition("flag", AstDefinitionKind::Parameter, "flag", 1, 5),
                        definition("keep", AstDefinitionKind::Parameter, "keep", 6, 10),
                        definition("stop", AstDefinitionKind::Parameter, "stop", 11, 15),
                        definition("skip", AstDefinitionKind::Parameter, "skip", 16, 20),
                        definition("done", AstDefinitionKind::Parameter, "done", 21, 25),
                        definition("x", AstDefinitionKind::Variable, "x = 0", 10, 15),
                        definition("x", AstDefinitionKind::Variable, "x = 1", 35, 40),
                        definition("x", AstDefinitionKind::Variable, "x = 2", 65, 70),
                        definition("y", AstDefinitionKind::Variable, "y = x", 95, 103),
                        definition("x", AstDefinitionKind::Variable, "x = 4", 130, 135),
                        definition("x", AstDefinitionKind::Variable, "x = 44", 145, 151),
                        definition("x", AstDefinitionKind::Variable, "x = 5", 160, 165),
                        definition("x", AstDefinitionKind::Variable, "x = 55", 175, 181),
                        definition("x", AstDefinitionKind::Variable, "x = x + 1", 211, 220),
                        definition(
                            "after_loop",
                            AstDefinitionKind::Variable,
                            "after_loop = x",
                            230,
                            238,
                        ),
                        definition("x", AstDefinitionKind::Variable, "x = 3", 275, 280),
                        definition(
                            "after_done",
                            AstDefinitionKind::Variable,
                            "after_done = x",
                            320,
                            328,
                        ),
                        definition("x", AstDefinitionKind::Variable, "x = 99", 365, 371),
                    ],
                    uses: vec![
                        use_fact("flag", UseKind::Identifier, 23, 27),
                        use_fact("x", UseKind::Identifier, 101, 102),
                        use_fact("keep", UseKind::Identifier, 116, 120),
                        use_fact("stop", UseKind::Identifier, 128, 132),
                        use_fact("skip", UseKind::Identifier, 158, 162),
                        use_fact("x", UseKind::Identifier, 215, 216),
                        use_fact("x", UseKind::Identifier, 236, 237),
                        use_fact("done", UseKind::Identifier, 263, 267),
                        use_fact("x", UseKind::Identifier, 297, 298),
                        use_fact("x", UseKind::Identifier, 326, 327),
                        use_fact("x", UseKind::Identifier, 352, 353),
                    ],
                    returns: vec![
                        ReturnAst {
                            value: Some("x".to_string()),
                            owner_id: "sample:process".to_string(),
                            source_span: span(290, 300),
                        },
                        ReturnAst {
                            value: Some("x".to_string()),
                            owner_id: "sample:process".to_string(),
                            source_span: span(345, 355),
                        },
                    ],
                    field_accesses: Vec::new(),
                    index_accesses: Vec::new(),
                    source_span: span(0, 380),
                }],
                statements: Vec::new(),
                expressions: Vec::new(),
                conditions: Vec::new(),
                definitions: Vec::new(),
                uses: Vec::new(),
                returns: Vec::new(),
                raises: Vec::new(),
                field_accesses: Vec::new(),
                index_accesses: Vec::new(),
                    parse_errors: Vec::new(),
                }],
        }
    }

    fn sg073_expressions() -> Vec<ExpressionAst> {
        vec![
            expression(AstExpressionKind::Assignment, "obj.value = seed", 30, 46),
            expression(AstExpressionKind::FieldAccess, "obj.value", 30, 39),
            expression(AstExpressionKind::Identifier, "obj", 30, 33),
            expression(AstExpressionKind::Identifier, "value", 34, 39),
            expression(AstExpressionKind::Identifier, "seed", 42, 46),
            expression(AstExpressionKind::Assignment, "read = obj.value", 50, 66),
            expression(AstExpressionKind::Identifier, "read", 50, 54),
            expression(AstExpressionKind::FieldAccess, "obj.value", 57, 66),
            expression(AstExpressionKind::Identifier, "obj", 57, 60),
            expression(AstExpressionKind::Identifier, "value", 61, 66),
            expression(AstExpressionKind::Assignment, "items[index] = read", 70, 89),
            expression(AstExpressionKind::IndexAccess, "items[index]", 70, 82),
            expression(AstExpressionKind::Identifier, "items", 70, 75),
            expression(AstExpressionKind::Identifier, "index", 76, 81),
            expression(AstExpressionKind::Identifier, "read", 85, 89),
            expression(AstExpressionKind::Assignment, "out = items[index]", 95, 113),
            expression(AstExpressionKind::Identifier, "out", 95, 98),
            expression(AstExpressionKind::IndexAccess, "items[index]", 101, 113),
            expression(AstExpressionKind::Identifier, "items", 101, 106),
            expression(AstExpressionKind::Identifier, "index", 107, 112),
            expression(
                AstExpressionKind::Assignment,
                "alias_read = alias.value",
                120,
                144,
            ),
            expression(AstExpressionKind::Identifier, "alias_read", 120, 130),
            expression(AstExpressionKind::FieldAccess, "alias.value", 133, 144),
            expression(AstExpressionKind::Identifier, "alias", 133, 138),
            expression(AstExpressionKind::Identifier, "value", 139, 144),
        ]
    }

    fn sg071_expressions() -> Vec<ExpressionAst> {
        vec![
            expression(AstExpressionKind::Identifier, "flag", 23, 27),
            expression(AstExpressionKind::Assignment, "x = 0", 10, 15),
            expression(AstExpressionKind::Identifier, "x", 10, 11),
            expression(AstExpressionKind::Literal, "0", 14, 15),
            expression(AstExpressionKind::Assignment, "x = 1", 35, 40),
            expression(AstExpressionKind::Identifier, "x", 35, 36),
            expression(AstExpressionKind::Literal, "1", 39, 40),
            expression(AstExpressionKind::Assignment, "x = 2", 65, 70),
            expression(AstExpressionKind::Identifier, "x", 65, 66),
            expression(AstExpressionKind::Literal, "2", 69, 70),
            expression(AstExpressionKind::Assignment, "y = x", 95, 103),
            expression(AstExpressionKind::Identifier, "y", 95, 96),
            expression(AstExpressionKind::Identifier, "x", 101, 102),
            expression(AstExpressionKind::Identifier, "keep", 116, 120),
            expression(AstExpressionKind::Identifier, "stop", 128, 132),
            expression(AstExpressionKind::Assignment, "x = 4", 130, 135),
            expression(AstExpressionKind::Identifier, "x", 130, 131),
            expression(AstExpressionKind::Literal, "4", 134, 135),
            expression(AstExpressionKind::Assignment, "x = 44", 145, 151),
            expression(AstExpressionKind::Identifier, "x", 145, 146),
            expression(AstExpressionKind::Literal, "44", 149, 151),
            expression(AstExpressionKind::Identifier, "skip", 158, 162),
            expression(AstExpressionKind::Assignment, "x = 5", 160, 165),
            expression(AstExpressionKind::Identifier, "x", 160, 161),
            expression(AstExpressionKind::Literal, "5", 164, 165),
            expression(AstExpressionKind::Assignment, "x = 55", 175, 181),
            expression(AstExpressionKind::Identifier, "x", 175, 176),
            expression(AstExpressionKind::Literal, "55", 179, 181),
            expression(AstExpressionKind::Assignment, "x = x + 1", 211, 220),
            expression(AstExpressionKind::Identifier, "x", 211, 212),
            expression(AstExpressionKind::BinaryOperator, "x + 1", 215, 220),
            expression(AstExpressionKind::Identifier, "x", 215, 216),
            expression(AstExpressionKind::Literal, "1", 219, 220),
            expression(AstExpressionKind::Assignment, "after_loop = x", 230, 238),
            expression(AstExpressionKind::Identifier, "after_loop", 230, 232),
            expression(AstExpressionKind::Identifier, "x", 236, 237),
            expression(AstExpressionKind::Identifier, "done", 263, 267),
            expression(AstExpressionKind::Assignment, "x = 3", 275, 280),
            expression(AstExpressionKind::Identifier, "x", 275, 276),
            expression(AstExpressionKind::Literal, "3", 279, 280),
            expression(AstExpressionKind::Identifier, "x", 297, 298),
            expression(AstExpressionKind::Assignment, "after_done = x", 320, 328),
            expression(AstExpressionKind::Identifier, "after_done", 320, 322),
            expression(AstExpressionKind::Identifier, "x", 326, 327),
            expression(AstExpressionKind::Identifier, "x", 352, 353),
            expression(AstExpressionKind::Assignment, "x = 99", 365, 371),
            expression(AstExpressionKind::Identifier, "x", 365, 366),
            expression(AstExpressionKind::Literal, "99", 369, 371),
        ]
    }

    fn expressions() -> Vec<ExpressionAst> {
        vec![
            expression(
                AstExpressionKind::Assignment,
                "total = items[index] + 1",
                20,
                45,
            ),
            expression(AstExpressionKind::Identifier, "total", 20, 25),
            expression(
                AstExpressionKind::BinaryOperator,
                "items[index] + 1",
                28,
                44,
            ),
            expression(AstExpressionKind::IndexAccess, "items[index]", 28, 40),
            expression(AstExpressionKind::Identifier, "items", 28, 33),
            expression(AstExpressionKind::Identifier, "index", 34, 39),
            expression(AstExpressionKind::Literal, "1", 43, 44),
            expression(AstExpressionKind::Assignment, "field = obj.value", 46, 68),
            expression(AstExpressionKind::Identifier, "field", 46, 51),
            expression(AstExpressionKind::FieldAccess, "obj.value", 54, 63),
            expression(AstExpressionKind::Identifier, "obj", 54, 57),
            expression(AstExpressionKind::Identifier, "value", 58, 63),
            expression(
                AstExpressionKind::Assignment,
                "sent = send(total, field)",
                69,
                90,
            ),
            expression(AstExpressionKind::Identifier, "sent", 69, 73),
            expression(AstExpressionKind::Call, "send(total, field)", 76, 88),
            expression(AstExpressionKind::Identifier, "send", 76, 80),
            expression(AstExpressionKind::Identifier, "total", 81, 86),
            expression(AstExpressionKind::Identifier, "field", 87, 88),
            expression(AstExpressionKind::Identifier, "total", 97, 102),
            expression(AstExpressionKind::Assignment, "total = total + 1", 100, 125),
            expression(AstExpressionKind::Identifier, "total", 100, 105),
            expression(AstExpressionKind::BinaryOperator, "total + 1", 108, 117),
            expression(AstExpressionKind::Identifier, "total", 108, 113),
            expression(AstExpressionKind::Literal, "1", 116, 117),
            expression(AstExpressionKind::Identifier, "sent", 138, 142),
        ]
    }

    fn param(name: &str, start: usize, end: usize) -> ParamAst {
        ParamAst {
            name: name.to_string(),
            text: name.to_string(),
            source_span: span(start, end),
        }
    }

    fn statement(kind: AstStatementKind, text: &str, start: usize, end: usize) -> StatementAst {
        StatementAst {
            kind,
            text: text.to_string(),
            owner_id: "sample:process".to_string(),
            source_span: span(start, end),
        }
    }

    fn expression(kind: AstExpressionKind, text: &str, start: usize, end: usize) -> ExpressionAst {
        ExpressionAst {
            kind,
            text: text.to_string(),
            owner_id: "sample:process".to_string(),
            source_span: span(start, end),
        }
    }

    fn condition(kind: ConditionKind, text: &str, start: usize, end: usize) -> ConditionAst {
        ConditionAst {
            kind,
            text: text.to_string(),
            owner_id: "sample:process".to_string(),
            source_span: span(start, end),
        }
    }

    fn definition(
        name: &str,
        kind: AstDefinitionKind,
        text: &str,
        start: usize,
        end: usize,
    ) -> DefinitionAst {
        DefinitionAst {
            name: name.to_string(),
            kind,
            text: text.to_string(),
            owner_id: "sample:process".to_string(),
            source_span: span(start, end),
        }
    }

    fn use_fact(name: &str, kind: UseKind, start: usize, end: usize) -> UseAst {
        UseAst {
            name: name.to_string(),
            kind,
            owner_id: "sample:process".to_string(),
            source_span: span(start, end),
        }
    }

    fn span(start: usize, end: usize) -> SourceSpan {
        SourceSpan {
            start_byte: start,
            end_byte: end,
            start_row: start,
            start_column: 0,
            end_row: end,
            end_column: 0,
        }
    }
}
