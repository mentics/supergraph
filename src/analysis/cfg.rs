use std::collections::{BTreeMap, BTreeSet};

use crate::ast::{ConditionAst, RaiseAst, ReturnAst, SourceSpan, StatementAst};
use crate::supergraph::{
    self as sg, BasicBlockKind, Confidence, ControlFlowKind, ControlFlowNodeRole,
    ControlRegionKind, DiagnosticKind, EdgeFact, EdgeKind, ExpressionKind, FallthroughBehavior,
    NodeFact, NodeId, NodeKind, ProgramSupergraph, Severity, StatementKind, stable_id,
};

use super::{
    SemanticCallable, SemanticContext, callable_index::CallableIndex, edge_id, graph_edge,
    graph_node, inference_evidence,
    insert_edge, insert_node, node_owner, span_key,
};

const PRECISION: &str = "sg050-structured-recursive-cfg";

pub(crate) fn emit(graph: &mut ProgramSupergraph, context: &SemanticContext<'_>) {
    let index = CallableIndex::build(graph);
    for semantic in context.semantic_callables() {
        emit_callable(graph, &index, &semantic);
    }
}

pub(crate) fn entry_node_id(callable_id: &str) -> NodeId {
    stable_id("cfg-node", &[callable_id, "entry"])
}

pub(crate) fn exit_node_id(callable_id: &str) -> NodeId {
    normal_exit_node_id(callable_id)
}

pub(crate) fn normal_exit_node_id(callable_id: &str) -> NodeId {
    stable_id("cfg-node", &[callable_id, "normal-exit"])
}

pub(crate) fn exceptional_exit_node_id(callable_id: &str) -> NodeId {
    stable_id("cfg-node", &[callable_id, "exceptional-exit"])
}

pub(crate) fn statement_node_id(callable_id: &str, statement: &StatementAst) -> NodeId {
    semantic_node_id(
        callable_id,
        "statement",
        statement.source_span,
        &statement.text,
    )
}

pub(crate) fn condition_node_id(callable_id: &str, condition: &ConditionAst) -> NodeId {
    semantic_node_id(
        callable_id,
        "condition",
        condition.source_span,
        &condition.text,
    )
}

pub(crate) fn return_node_id(callable_id: &str, return_fact: &ReturnAst) -> NodeId {
    semantic_node_id(
        callable_id,
        "return",
        return_fact.source_span,
        return_fact.value.as_deref().unwrap_or("return"),
    )
}

pub(crate) fn raise_node_id(callable_id: &str, raise: &RaiseAst) -> NodeId {
    semantic_node_id(callable_id, "raise", raise.source_span, &raise.text)
}

fn emit_callable(
    graph: &mut ProgramSupergraph,
    index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
) {
    let callable_id = semantic.callable().callable_id.as_str();
    let owner = node_owner(semantic);
    let entry_id = entry_node_id(callable_id);
    let normal_exit_id = exit_node_id(callable_id);
    let exceptional_exit_id = exceptional_exit_node_id(callable_id);

    insert_node(
        graph,
        graph_node(
            entry_id.clone(),
            NodeKind::ControlFlow,
            owner.clone(),
            semantic.callable().body_span,
            Confidence::Exact,
            inference_evidence("callable-local CFG entry"),
            NodeFact::ControlFlow(sg::ControlFlowNode {
                cfg_node_id: entry_id.clone(),
                callable_id: callable_id.to_string(),
                role: ControlFlowNodeRole::Entry,
                label: "entry".to_string(),
                semantic_kind: None,
            }),
        ),
    );
    insert_node(
        graph,
        graph_node(
            normal_exit_id.clone(),
            NodeKind::ControlFlow,
            owner.clone(),
            semantic.callable().body_span,
            Confidence::Exact,
            inference_evidence("callable-local CFG normal exit"),
            NodeFact::ControlFlow(sg::ControlFlowNode {
                cfg_node_id: normal_exit_id.clone(),
                callable_id: callable_id.to_string(),
                role: ControlFlowNodeRole::Exit,
                label: "normal exit".to_string(),
                semantic_kind: Some("NormalExit".to_string()),
            }),
        ),
    );
    insert_node(
        graph,
        graph_node(
            exceptional_exit_id.clone(),
            NodeKind::ControlFlow,
            owner.clone(),
            semantic.callable().body_span,
            Confidence::Exact,
            inference_evidence("callable-local CFG exceptional exit"),
            NodeFact::ControlFlow(sg::ControlFlowNode {
                cfg_node_id: exceptional_exit_id.clone(),
                callable_id: callable_id.to_string(),
                role: ControlFlowNodeRole::Exit,
                label: "exceptional exit".to_string(),
                semantic_kind: Some("ExceptionalExit".to_string()),
            }),
        ),
    );

    let model = CfgModel::new(graph, index, semantic);
    insert_cfg_nodes(graph, semantic, &model);
    insert_basic_block_nodes(graph, semantic, &model);

    let top_level = model.statement_list(None);
    if top_level.is_empty() {
        add_cfg_edge(
            graph,
            semantic,
            &entry_id,
            &normal_exit_id,
            ControlFlowKind::Exit,
            None,
        );
        return;
    }

    let mut lowering = Lowering::new(graph, semantic, &model);
    let result = lowering.lower_statement_list(&top_level, &LoweringEnv::default());
    if let Some(first) = result.entry {
        add_cfg_edge(
            lowering.graph,
            semantic,
            &entry_id,
            &first,
            ControlFlowKind::Entry,
            None,
        );
    }
    for exit in result.exits {
        let target_id = exit.target_exit_id(&normal_exit_id, &exceptional_exit_id);
        add_cfg_edge_with_metadata(
            lowering.graph,
            semantic,
            &exit.node_id,
            target_id,
            ControlFlowKind::Exit,
            exit.kind.outcome(),
            exit.kind.branch_arm().cloned(),
            Some(exit.span),
        );
    }
}

fn cfg_node(
    semantic: &SemanticCallable<'_>,
    node_id: NodeId,
    span: SourceSpan,
    role: ControlFlowNodeRole,
    label: String,
    semantic_kind: Option<String>,
    evidence: &str,
) -> sg::GraphNode {
    graph_node(
        node_id.clone(),
        NodeKind::ControlFlow,
        node_owner(semantic),
        Some(span),
        Confidence::Exact,
        inference_evidence(evidence),
        NodeFact::ControlFlow(sg::ControlFlowNode {
            cfg_node_id: node_id,
            callable_id: semantic.callable().callable_id.clone(),
            role,
            label,
            semantic_kind,
        }),
    )
}

fn add_cfg_edge(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    source_id: &str,
    target_id: &str,
    flow_kind: ControlFlowKind,
    span: Option<SourceSpan>,
) {
    add_cfg_edge_with_metadata(
        graph,
        semantic,
        source_id,
        target_id,
        flow_kind,
        default_outcome_for_flow_kind(flow_kind),
        None,
        span,
    );
}

fn add_cfg_edge_with_metadata(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    source_id: &str,
    target_id: &str,
    flow_kind: ControlFlowKind,
    outcome: sg::ControlFlowOutcome,
    branch_arm: Option<sg::ControlFlowBranchArm>,
    span: Option<SourceSpan>,
) {
    let outcome_key = branch_arm
        .as_ref()
        .map(|arm| format!("{outcome:?}:{}:{}", arm.ordinal, arm.label))
        .unwrap_or_else(|| format!("{outcome:?}"));
    insert_edge(
        graph,
        graph_edge(
            edge_id(
                "control-flow",
                source_id,
                target_id,
                &format!("{flow_kind:?}:{outcome_key}"),
            ),
            EdgeKind::ControlFlow,
            source_id.to_string(),
            target_id.to_string(),
            node_owner(semantic),
            span,
            Confidence::Probable,
            inference_evidence(PRECISION),
            EdgeFact::ControlFlow(sg::ControlFlow {
                callable_id: semantic.callable().callable_id.clone(),
                flow_kind,
                outcome,
                branch_arm,
                precision: PRECISION.to_string(),
            }),
        ),
    );
}

fn default_outcome_for_flow_kind(flow_kind: ControlFlowKind) -> sg::ControlFlowOutcome {
    match flow_kind {
        ControlFlowKind::Entry => sg::ControlFlowOutcome::Entry,
        ControlFlowKind::Exit => sg::ControlFlowOutcome::Exit,
        ControlFlowKind::Sequential => sg::ControlFlowOutcome::Fallthrough,
        ControlFlowKind::Branch => sg::ControlFlowOutcome::Arm,
        ControlFlowKind::LoopBack => sg::ControlFlowOutcome::LoopBack,
    }
}

fn semantic_node_id(callable_id: &str, role: &str, span: SourceSpan, label: &str) -> NodeId {
    stable_id("cfg-node", &[callable_id, role, &span_key(span), label])
}

#[derive(Debug)]
struct CfgModel {
    statements: BTreeMap<NodeId, StatementInfo>,
    children_by_parent: BTreeMap<Option<NodeId>, Vec<NodeId>>,
    conditions_by_statement: BTreeMap<NodeId, ConditionInfo>,
    exception_conditions_by_statement: BTreeMap<NodeId, ConditionInfo>,
    expression_controls_by_statement: BTreeMap<NodeId, Vec<ExpressionControlInfo>>,
}

impl CfgModel {
    fn new(graph: &ProgramSupergraph, index: &CallableIndex, semantic: &SemanticCallable<'_>) -> Self {
        let ast_statements_by_span = semantic
            .statements()
            .iter()
            .filter(|statement| statement.owner_id == semantic.owner_id())
            .map(|statement| (statement.source_span, statement))
            .collect::<BTreeMap<_, _>>();
        let mut statements = index
            .nodes(graph, &semantic.callable().callable_id)
            .filter_map(|node| match &node.fact {
                NodeFact::Statement(statement) => {
                    let span = node.span?;
                    let ast = ast_statements_by_span.get(&span)?;
                    Some((
                        statement.statement_id.clone(),
                        StatementInfo {
                            statement_id: statement.statement_id.clone(),
                            cfg_node_id: statement_node_id(&semantic.callable().callable_id, ast),
                            parent_statement_id: statement.parent_statement_id.clone(),
                            kind: statement.kind,
                            ordinal: statement.ordinal,
                            text: ast.text.clone(),
                            span,
                        },
                    ))
                }
                _ => None,
            })
            .collect::<BTreeMap<_, _>>();

        let mut children_by_parent = BTreeMap::<Option<NodeId>, Vec<NodeId>>::new();
        for statement in statements.values() {
            children_by_parent
                .entry(statement.parent_statement_id.clone())
                .or_default()
                .push(statement.statement_id.clone());
        }
        for child_ids in children_by_parent.values_mut() {
            child_ids.sort_by(|left, right| {
                statement_sort_key(&statements[left]).cmp(&statement_sort_key(&statements[right]))
            });
            child_ids.dedup();
        }

        let ast_conditions_by_span = semantic
            .conditions()
            .iter()
            .filter(|condition| condition.owner_id == semantic.owner_id())
            .map(|condition| (condition.source_span, condition))
            .collect::<BTreeMap<_, _>>();
        let mut conditions_by_statement = BTreeMap::new();
        let mut exception_conditions_by_statement = BTreeMap::new();
        for node in index.nodes(graph, &semantic.callable().callable_id) {
            let NodeFact::Condition(condition) = &node.fact else {
                continue;
            };
            let Some(statement_id) = &condition.statement_id else {
                continue;
            };
            let Some(span) = node.span else {
                continue;
            };
            let cfg_node_id = ast_conditions_by_span
                .get(&span)
                .map(|ast| condition_node_id(&semantic.callable().callable_id, ast))
                .unwrap_or_else(|| {
                    structured_condition_node_id(&semantic.callable().callable_id, condition, span)
                });
            let info = ConditionInfo {
                cfg_node_id,
                kind: condition.kind,
                regions: condition.regions.clone(),
                fallthrough: condition.fallthrough,
                span,
            };
            if condition.kind == sg::ConditionKind::ExceptionRegion {
                exception_conditions_by_statement.insert(statement_id.clone(), info);
            } else {
                conditions_by_statement.insert(statement_id.clone(), info);
            }
        }

        let mut expression_controls_by_statement =
            BTreeMap::<NodeId, Vec<ExpressionControlInfo>>::new();
        for node in index.nodes(graph, &semantic.callable().callable_id) {
            let NodeFact::Expression(expression) = &node.fact else {
                continue;
            };
            let Some(statement_id) = &expression.statement_id else {
                continue;
            };
            let Some(span) = node.span else {
                continue;
            };
            if !expression_is_control_region(expression) {
                continue;
            }
            expression_controls_by_statement
                .entry(statement_id.clone())
                .or_default()
                .push(ExpressionControlInfo {
                    expression_id: expression.expression_id.clone(),
                    cfg_node_id: expression_control_node_id(
                        &semantic.callable().callable_id,
                        expression,
                    ),
                    kind: expression.kind,
                    label: expression
                        .original_text
                        .clone()
                        .or_else(|| expression.normalized.canonical.clone())
                        .unwrap_or_else(|| "expression-control".to_string()),
                    span,
                });
        }
        for controls in expression_controls_by_statement.values_mut() {
            controls.sort_by_key(|control| {
                (
                    control.span.start_byte,
                    control.span.end_byte,
                    control.cfg_node_id.clone(),
                )
            });
            controls.dedup_by(|left, right| left.expression_id == right.expression_id);
        }

        statements.retain(|_, statement| ast_statements_by_span.contains_key(&statement.span));

        Self {
            statements,
            children_by_parent,
            conditions_by_statement,
            exception_conditions_by_statement,
            expression_controls_by_statement,
        }
    }

    fn statement_list(&self, parent: Option<&str>) -> Vec<NodeId> {
        self.children_by_parent
            .get(&parent.map(str::to_string))
            .cloned()
            .unwrap_or_default()
    }

    fn region_statement_list(
        &self,
        controller_id: &str,
        region: &sg::ControlRegion,
    ) -> Vec<NodeId> {
        let region_ids = region
            .statement_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut roots = region
            .statement_ids
            .iter()
            .filter_map(|statement_id| self.statements.get(statement_id))
            .filter(|statement| {
                statement
                    .parent_statement_id
                    .as_deref()
                    .is_none_or(|parent| parent == controller_id || !region_ids.contains(parent))
            })
            .map(|statement| statement.statement_id.clone())
            .collect::<Vec<_>>();
        roots.sort_by(|left, right| {
            statement_sort_key(&self.statements[left])
                .cmp(&statement_sort_key(&self.statements[right]))
        });
        roots.dedup();
        roots
    }
}

struct Lowering<'a, 'b> {
    graph: &'a mut ProgramSupergraph,
    semantic: &'b SemanticCallable<'b>,
    model: &'b CfgModel,
}

impl<'a, 'b> Lowering<'a, 'b> {
    fn new(
        graph: &'a mut ProgramSupergraph,
        semantic: &'b SemanticCallable<'b>,
        model: &'b CfgModel,
    ) -> Self {
        Self {
            graph,
            semantic,
            model,
        }
    }

    fn lower_statement_list(
        &mut self,
        statement_ids: &[NodeId],
        env: &LoweringEnv,
    ) -> LoweringResult {
        let mut entry = None;
        let mut pending = Vec::<OpenExit>::new();
        let mut carried = Vec::<OpenExit>::new();
        let mut has_reachable_path = true;
        let mut blocking_causes = Vec::<TerminalCause>::new();

        for statement_id in statement_ids {
            let Some(statement) = self.model.statements.get(statement_id) else {
                continue;
            };
            let result = self.lower_statement(statement, env);
            if !has_reachable_path {
                self.emit_unreachable_statement(statement, &blocking_causes);
                continue;
            }
            if entry.is_none() {
                entry = result.entry.clone();
            }
            if let Some(next_entry) = &result.entry {
                connect_normal_exits(self.graph, self.semantic, &pending, next_entry);
            }
            carried.extend(non_normal_exits(&pending));
            pending = result.exits;
            has_reachable_path = pending.iter().any(|exit| exit.kind.is_normal());
            if has_reachable_path {
                blocking_causes.clear();
            } else {
                blocking_causes = result.terminal_causes.clone();
                if blocking_causes.is_empty() {
                    blocking_causes = terminal_causes_from_exits(self.model, &pending);
                }
            }
        }
        carried.extend(pending);

        LoweringResult {
            entry,
            exits: carried,
            terminal_causes: blocking_causes,
        }
    }

    fn emit_unreachable_statement(&mut self, statement: &StatementInfo, causes: &[TerminalCause]) {
        let cause_summary = terminal_cause_summary(causes);
        let message =
            format!("statement is unreachable because {cause_summary} prevents normal fallthrough");
        let mut related = vec![
            statement.statement_id.clone(),
            statement.cfg_node_id.clone(),
        ];
        for cause in causes {
            related.push(cause.statement_id.clone());
            related.push(cause.cfg_node_id.clone());
        }
        related.sort();
        related.dedup();
        insert_node(
            self.graph,
            graph_node(
                stable_id(
                    "diagnostic",
                    &[
                        "unreachable-statement",
                        &self.semantic.callable().callable_id,
                        &statement.statement_id,
                        &related.join("|"),
                    ],
                ),
                NodeKind::Diagnostic,
                node_owner(self.semantic),
                Some(statement.span),
                Confidence::Exact,
                inference_evidence(format!("SG-054 unreachable statement: {cause_summary}")),
                NodeFact::Diagnostic(sg::Diagnostic {
                    diagnostic_id: stable_id(
                        "diagnostic",
                        &[
                            "unreachable-statement",
                            &self.semantic.callable().callable_id,
                            &statement.statement_id,
                            &related.join("|"),
                        ],
                    ),
                    kind: DiagnosticKind::UnreachableStatement,
                    severity: Severity::Warning,
                    message,
                    artifact_id: self.semantic.artifact().artifact_id.clone().into(),
                    span: Some(statement.span),
                    related,
                }),
            ),
        );
    }

    fn lower_statement(&mut self, statement: &StatementInfo, env: &LoweringEnv) -> LoweringResult {
        self.lower_expression_controls(statement);

        match statement.kind {
            StatementKind::Branch => self.lower_branch(statement, env),
            StatementKind::Loop => self.lower_loop(statement, env),
            StatementKind::Return => self.lower_return(statement),
            StatementKind::Raise | StatementKind::Throw => self.lower_raise(statement, env),
            StatementKind::Break => self.lower_break(statement),
            StatementKind::Continue => self.lower_continue(statement, env),
            StatementKind::Try => self.lower_try(statement, env),
            StatementKind::Catch | StatementKind::Finally => {
                self.lower_container_statement(statement, env)
            }
            _ => self.lower_plain_statement(statement),
        }
    }

    fn lower_plain_statement(&mut self, statement: &StatementInfo) -> LoweringResult {
        let entry = self.entry_for_statement(statement);
        LoweringResult {
            entry: Some(entry),
            exits: vec![OpenExit::normal(
                statement.cfg_node_id.clone(),
                ControlFlowKind::Sequential,
                statement.span,
            )],
            terminal_causes: Vec::new(),
        }
    }

    fn lower_container_statement(
        &mut self,
        statement: &StatementInfo,
        env: &LoweringEnv,
    ) -> LoweringResult {
        let entry = self.entry_for_statement(statement);
        let child_ids = self.model.statement_list(Some(&statement.statement_id));
        if child_ids.is_empty() {
            return LoweringResult {
                entry: Some(entry),
                exits: vec![OpenExit::normal(
                    statement.cfg_node_id.clone(),
                    ControlFlowKind::Sequential,
                    statement.span,
                )],
                terminal_causes: Vec::new(),
            };
        }

        let result = self.lower_statement_list(&child_ids, env);
        if let Some(child_entry) = &result.entry {
            add_cfg_edge(
                self.graph,
                self.semantic,
                &statement.cfg_node_id,
                child_entry,
                ControlFlowKind::Sequential,
                Some(statement.span),
            );
        }

        LoweringResult {
            entry: Some(entry),
            exits: result.exits,
            terminal_causes: result.terminal_causes,
        }
    }

    fn lower_branch(&mut self, statement: &StatementInfo, env: &LoweringEnv) -> LoweringResult {
        let Some(condition) = self
            .model
            .conditions_by_statement
            .get(&statement.statement_id)
        else {
            return self.lower_plain_statement(statement);
        };
        let entry = self.entry_for_statement(statement);
        add_cfg_edge(
            self.graph,
            self.semantic,
            &statement.cfg_node_id,
            &condition.cfg_node_id,
            ControlFlowKind::Sequential,
            Some(statement.span),
        );

        let mut exits = Vec::new();
        let mut has_else_region = false;
        for (ordinal, region) in condition
            .regions
            .iter()
            .filter(|region| {
                matches!(
                    region.kind,
                    ControlRegionKind::BranchBody | ControlRegionKind::ElseBody
                )
            })
            .enumerate()
        {
            if region.kind == ControlRegionKind::ElseBody {
                has_else_region = true;
            }
            let outcome = branch_region_outcome(region.kind);
            let branch_arm = branch_arm(region, ordinal);
            let region_ids = self
                .model
                .region_statement_list(&statement.statement_id, region);
            let result = self.lower_statement_list(&region_ids, env);
            if let Some(region_entry) = &result.entry {
                add_cfg_edge_with_metadata(
                    self.graph,
                    self.semantic,
                    &condition.cfg_node_id,
                    region_entry,
                    ControlFlowKind::Branch,
                    outcome,
                    Some(branch_arm.clone()),
                    Some(condition.span),
                );
            } else {
                exits.push(OpenExit::normal_with_metadata(
                    condition.cfg_node_id.clone(),
                    ControlFlowKind::Branch,
                    outcome,
                    Some(branch_arm.clone()),
                    condition.span,
                ));
            }
            exits.extend(mark_normal_exits(result.exits, outcome, Some(branch_arm)));
        }
        if !has_else_region && condition.fallthrough != FallthroughBehavior::DoesNotFallThrough {
            exits.push(OpenExit::normal_with_metadata(
                condition.cfg_node_id.clone(),
                ControlFlowKind::Branch,
                sg::ControlFlowOutcome::False,
                None,
                condition.span,
            ));
        }

        let exits = self.materialize_merge(
            "branch",
            "branch merge",
            "BranchMerge",
            &statement.statement_id,
            statement.span,
            exits,
        );

        LoweringResult {
            entry: Some(entry),
            terminal_causes: terminal_causes_from_exits(self.model, &exits),
            exits,
        }
    }

    fn lower_loop(&mut self, statement: &StatementInfo, env: &LoweringEnv) -> LoweringResult {
        let Some(condition) = self
            .model
            .conditions_by_statement
            .get(&statement.statement_id)
        else {
            return self.lower_plain_statement(statement);
        };
        let entry = self.entry_for_statement(statement);
        add_cfg_edge(
            self.graph,
            self.semantic,
            &statement.cfg_node_id,
            &condition.cfg_node_id,
            ControlFlowKind::Sequential,
            Some(statement.span),
        );

        let mut loop_env = env.clone();
        loop_env.loop_condition_id = Some(condition.cfg_node_id.clone());

        let mut propagated = Vec::new();
        for (ordinal, region) in condition
            .regions
            .iter()
            .filter(|region| region.kind == ControlRegionKind::LoopBody)
            .enumerate()
        {
            let branch_arm = branch_arm(region, ordinal);
            let region_ids = self
                .model
                .region_statement_list(&statement.statement_id, region);
            let result = self.lower_statement_list(&region_ids, &loop_env);
            if let Some(region_entry) = &result.entry {
                add_cfg_edge_with_metadata(
                    self.graph,
                    self.semantic,
                    &condition.cfg_node_id,
                    region_entry,
                    ControlFlowKind::Branch,
                    sg::ControlFlowOutcome::True,
                    Some(branch_arm),
                    Some(condition.span),
                );
            }
            for exit in result.exits {
                match exit.kind {
                    OpenExitKind::Normal { .. } | OpenExitKind::Continue => {
                        let outcome = if matches!(exit.kind, OpenExitKind::Continue) {
                            sg::ControlFlowOutcome::Continue
                        } else {
                            sg::ControlFlowOutcome::LoopBack
                        };
                        add_cfg_edge_with_metadata(
                            self.graph,
                            self.semantic,
                            &exit.node_id,
                            &condition.cfg_node_id,
                            ControlFlowKind::LoopBack,
                            outcome,
                            None,
                            Some(exit.span),
                        );
                    }
                    OpenExitKind::Break => {
                        propagated.push(OpenExit::normal_with_metadata(
                            exit.node_id,
                            ControlFlowKind::Branch,
                            sg::ControlFlowOutcome::Break,
                            None,
                            exit.span,
                        ));
                    }
                    OpenExitKind::Return | OpenExitKind::Raise => propagated.push(exit),
                }
            }
        }
        propagated.push(OpenExit::normal_with_metadata(
            condition.cfg_node_id.clone(),
            ControlFlowKind::Branch,
            sg::ControlFlowOutcome::False,
            None,
            condition.span,
        ));
        let propagated = self.materialize_merge(
            "loop-exit",
            "loop exit merge",
            "LoopExitMerge",
            &statement.statement_id,
            statement.span,
            propagated,
        );

        LoweringResult {
            entry: Some(entry),
            exits: propagated,
            terminal_causes: Vec::new(),
        }
    }

    fn lower_return(&mut self, statement: &StatementInfo) -> LoweringResult {
        let entry = self.entry_for_statement(statement);
        if let Some(return_id) = self.return_node_for_statement(statement) {
            add_cfg_edge(
                self.graph,
                self.semantic,
                &statement.cfg_node_id,
                &return_id,
                ControlFlowKind::Sequential,
                Some(statement.span),
            );
            LoweringResult {
                entry: Some(entry),
                exits: vec![OpenExit {
                    node_id: return_id,
                    span: statement.span,
                    kind: OpenExitKind::Return,
                }],
                terminal_causes: vec![TerminalCause::from_statement(statement)],
            }
        } else {
            LoweringResult {
                entry: Some(entry),
                exits: vec![OpenExit {
                    node_id: statement.cfg_node_id.clone(),
                    span: statement.span,
                    kind: OpenExitKind::Return,
                }],
                terminal_causes: vec![TerminalCause::from_statement(statement)],
            }
        }
    }

    fn lower_raise(&mut self, statement: &StatementInfo, env: &LoweringEnv) -> LoweringResult {
        let entry = self.entry_for_statement(statement);
        let raise_id = self
            .raise_node_for_statement(statement)
            .unwrap_or_else(|| statement.cfg_node_id.clone());
        if raise_id != statement.cfg_node_id {
            add_cfg_edge(
                self.graph,
                self.semantic,
                &statement.cfg_node_id,
                &raise_id,
                ControlFlowKind::Sequential,
                Some(statement.span),
            );
        }
        if let Some(handler) = &env.exception_handler {
            add_cfg_edge_with_metadata(
                self.graph,
                self.semantic,
                &raise_id,
                &handler.node_id,
                ControlFlowKind::Branch,
                sg::ControlFlowOutcome::Exception,
                None,
                Some(statement.span),
            );
            if handler.handles_exception {
                LoweringResult {
                    entry: Some(entry),
                    exits: Vec::new(),
                    terminal_causes: vec![TerminalCause::from_statement(statement)],
                }
            } else {
                LoweringResult {
                    entry: Some(entry),
                    exits: vec![OpenExit {
                        node_id: raise_id,
                        span: statement.span,
                        kind: OpenExitKind::Raise,
                    }],
                    terminal_causes: vec![TerminalCause::from_statement(statement)],
                }
            }
        } else {
            LoweringResult {
                entry: Some(entry),
                exits: vec![OpenExit {
                    node_id: raise_id,
                    span: statement.span,
                    kind: OpenExitKind::Raise,
                }],
                terminal_causes: vec![TerminalCause::from_statement(statement)],
            }
        }
    }

    fn lower_break(&mut self, statement: &StatementInfo) -> LoweringResult {
        LoweringResult {
            entry: Some(self.entry_for_statement(statement)),
            exits: vec![OpenExit {
                node_id: statement.cfg_node_id.clone(),
                span: statement.span,
                kind: OpenExitKind::Break,
            }],
            terminal_causes: vec![TerminalCause::from_statement(statement)],
        }
    }

    fn lower_continue(&mut self, statement: &StatementInfo, env: &LoweringEnv) -> LoweringResult {
        let entry = self.entry_for_statement(statement);
        if let Some(condition_id) = &env.loop_condition_id {
            add_cfg_edge_with_metadata(
                self.graph,
                self.semantic,
                &statement.cfg_node_id,
                condition_id,
                ControlFlowKind::LoopBack,
                sg::ControlFlowOutcome::Continue,
                None,
                Some(statement.span),
            );
            LoweringResult {
                entry: Some(entry),
                exits: Vec::new(),
                terminal_causes: vec![TerminalCause::from_statement(statement)],
            }
        } else {
            LoweringResult {
                entry: Some(entry),
                exits: vec![OpenExit {
                    node_id: statement.cfg_node_id.clone(),
                    span: statement.span,
                    kind: OpenExitKind::Continue,
                }],
                terminal_causes: vec![TerminalCause::from_statement(statement)],
            }
        }
    }

    fn lower_try(&mut self, statement: &StatementInfo, env: &LoweringEnv) -> LoweringResult {
        let Some(condition) = self
            .model
            .exception_conditions_by_statement
            .get(&statement.statement_id)
        else {
            return self.lower_plain_statement(statement);
        };
        let entry = self.entry_for_statement(statement);
        add_cfg_edge(
            self.graph,
            self.semantic,
            &statement.cfg_node_id,
            &condition.cfg_node_id,
            ControlFlowKind::Sequential,
            Some(statement.span),
        );

        let try_region = condition
            .regions
            .iter()
            .find(|region| region.kind == ControlRegionKind::TryBody);
        let catch_region = condition
            .regions
            .iter()
            .find(|region| region.kind == ControlRegionKind::CatchBody);
        let finally_region = condition
            .regions
            .iter()
            .find(|region| region.kind == ControlRegionKind::FinallyBody);

        let catch_entry = catch_region.and_then(|region| {
            self.model
                .region_statement_list(&statement.statement_id, region)
                .first()
                .and_then(|statement_id| self.model.statements.get(statement_id))
                .map(|statement| self.entry_for_statement(statement))
        });
        let finally_entry = finally_region.and_then(|region| {
            self.model
                .region_statement_list(&statement.statement_id, region)
                .first()
                .and_then(|statement_id| self.model.statements.get(statement_id))
                .map(|statement| self.entry_for_statement(statement))
        });
        let finally_entry_merge = finally_entry.as_ref().map(|_| {
            let node_id = merge_node_id(
                &self.semantic.callable().callable_id,
                "finally-entry",
                &statement.statement_id,
            );
            self.insert_merge_node(
                node_id.clone(),
                statement.span,
                "finally entry merge",
                "FinallyEntryMerge",
                "structured try/finally merge",
            );
            node_id
        });

        let mut protected_env = env.clone();
        protected_env.exception_handler = catch_entry
            .clone()
            .map(ExceptionHandler::catch)
            .or_else(|| finally_entry_merge.clone().map(ExceptionHandler::finally));

        let mut exits = Vec::new();
        if let Some(region) = try_region {
            let region_ids = self
                .model
                .region_statement_list(&statement.statement_id, region);
            let result = self.lower_statement_list(&region_ids, &protected_env);
            if let Some(region_entry) = &result.entry {
                add_cfg_edge_with_metadata(
                    self.graph,
                    self.semantic,
                    &condition.cfg_node_id,
                    region_entry,
                    ControlFlowKind::Branch,
                    sg::ControlFlowOutcome::Fallthrough,
                    Some(branch_arm(region, 0)),
                    Some(condition.span),
                );
            }
            exits.extend(result.exits);
        }

        if let Some(region) = catch_region {
            let region_ids = self
                .model
                .region_statement_list(&statement.statement_id, region);
            let mut catch_env = env.clone();
            catch_env.exception_handler =
                finally_entry_merge.clone().map(ExceptionHandler::finally);
            let result = self.lower_statement_list(&region_ids, &catch_env);
            if let Some(region_entry) = &result.entry {
                add_cfg_edge_with_metadata(
                    self.graph,
                    self.semantic,
                    &condition.cfg_node_id,
                    region_entry,
                    ControlFlowKind::Branch,
                    sg::ControlFlowOutcome::Exception,
                    Some(branch_arm(region, 1)),
                    Some(condition.span),
                );
            }
            exits.extend(mark_normal_exits(
                result.exits,
                sg::ControlFlowOutcome::Exception,
                Some(branch_arm(region, 1)),
            ));
        }

        if let Some(region) = finally_region {
            let pre_finally_exits = exits;
            let region_ids = self
                .model
                .region_statement_list(&statement.statement_id, region);
            let result = self.lower_statement_list(&region_ids, env);
            if let Some(finally_entry) = &result.entry {
                let finally_merge = finally_entry_merge
                    .clone()
                    .unwrap_or_else(|| finally_entry.clone());
                for exit in &pre_finally_exits {
                    add_cfg_edge_with_metadata(
                        self.graph,
                        self.semantic,
                        &exit.node_id,
                        &finally_merge,
                        ControlFlowKind::Branch,
                        sg::ControlFlowOutcome::Finally,
                        finally_region.map(|region| branch_arm(region, 2)),
                        Some(exit.span),
                    );
                }
                if pre_finally_exits.is_empty() {
                    add_cfg_edge_with_metadata(
                        self.graph,
                        self.semantic,
                        &condition.cfg_node_id,
                        &finally_merge,
                        ControlFlowKind::Branch,
                        sg::ControlFlowOutcome::Finally,
                        finally_region.map(|region| branch_arm(region, 2)),
                        Some(condition.span),
                    );
                }
                add_cfg_edge_with_metadata(
                    self.graph,
                    self.semantic,
                    &finally_merge,
                    finally_entry,
                    ControlFlowKind::Sequential,
                    sg::ControlFlowOutcome::Finally,
                    finally_region.map(|region| branch_arm(region, 2)),
                    Some(statement.span),
                );
                exits = exits_after_finally(pre_finally_exits, result.exits);
            } else {
                exits = pre_finally_exits;
            }
        }
        exits = self.materialize_merge(
            "exception",
            "exception region merge",
            "ExceptionMerge",
            &statement.statement_id,
            statement.span,
            exits,
        );

        LoweringResult {
            entry: Some(entry),
            terminal_causes: terminal_causes_from_exits(self.model, &exits),
            exits,
        }
    }

    fn lower_expression_controls(&mut self, statement: &StatementInfo) {
        let Some(controls) = self
            .model
            .expression_controls_by_statement
            .get(&statement.statement_id)
        else {
            return;
        };
        let mut previous: Option<&str> = None;
        for control in controls {
            if let Some(previous_id) = previous {
                add_cfg_edge(
                    self.graph,
                    self.semantic,
                    previous_id,
                    &control.cfg_node_id,
                    ControlFlowKind::Branch,
                    Some(control.span),
                );
            }
            previous = Some(&control.cfg_node_id);
        }
        if let Some(last) = controls.last() {
            add_cfg_edge(
                self.graph,
                self.semantic,
                &last.cfg_node_id,
                &statement.cfg_node_id,
                ControlFlowKind::Branch,
                Some(last.span),
            );
            add_cfg_edge(
                self.graph,
                self.semantic,
                &last.cfg_node_id,
                &statement.cfg_node_id,
                ControlFlowKind::Sequential,
                Some(last.span),
            );
        }
    }

    fn entry_for_statement(&self, statement: &StatementInfo) -> NodeId {
        self.model
            .expression_controls_by_statement
            .get(&statement.statement_id)
            .and_then(|controls| controls.first())
            .map(|control| control.cfg_node_id.clone())
            .unwrap_or_else(|| statement.cfg_node_id.clone())
    }

    fn return_node_for_statement(&self, statement: &StatementInfo) -> Option<NodeId> {
        self.semantic
            .returns()
            .iter()
            .filter(|return_fact| return_fact.owner_id == self.semantic.owner_id())
            .find(|return_fact| return_fact.source_span == statement.span)
            .map(|return_fact| return_node_id(&self.semantic.callable().callable_id, return_fact))
    }

    fn raise_node_for_statement(&self, statement: &StatementInfo) -> Option<NodeId> {
        self.semantic
            .raises()
            .iter()
            .filter(|raise| raise.owner_id == self.semantic.owner_id())
            .find(|raise| raise.source_span == statement.span)
            .map(|raise| raise_node_id(&self.semantic.callable().callable_id, raise))
    }

    fn materialize_merge(
        &mut self,
        kind: &str,
        label: &str,
        semantic_kind: &str,
        anchor_id: &str,
        span: SourceSpan,
        exits: Vec<OpenExit>,
    ) -> Vec<OpenExit> {
        let mut normal_exits = Vec::new();
        let mut propagated = Vec::new();
        for exit in exits {
            if exit.kind.is_normal() {
                normal_exits.push(exit);
            } else {
                propagated.push(exit);
            }
        }
        if normal_exits.is_empty() {
            return propagated;
        }

        let merge_id = merge_node_id(&self.semantic.callable().callable_id, kind, anchor_id);
        self.insert_merge_node(
            merge_id.clone(),
            span,
            label,
            semantic_kind,
            "structured CFG rejoin point",
        );
        for exit in &normal_exits {
            add_cfg_edge_with_metadata(
                self.graph,
                self.semantic,
                &exit.node_id,
                &merge_id,
                exit.kind.flow_kind(),
                exit.kind.outcome(),
                exit.kind.branch_arm().cloned(),
                Some(exit.span),
            );
        }
        propagated.push(OpenExit::normal(
            merge_id,
            ControlFlowKind::Sequential,
            span,
        ));
        propagated
    }

    fn insert_merge_node(
        &mut self,
        node_id: NodeId,
        span: SourceSpan,
        label: &str,
        semantic_kind: &str,
        evidence: &str,
    ) {
        insert_node(
            self.graph,
            cfg_node(
                self.semantic,
                node_id,
                span,
                ControlFlowNodeRole::Merge,
                label.to_string(),
                Some(semantic_kind.to_string()),
                evidence,
            ),
        );
    }
}

fn insert_cfg_nodes(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    model: &CfgModel,
) {
    for statement in model.statements.values() {
        let role = match statement.kind {
            StatementKind::Return => ControlFlowNodeRole::Statement,
            StatementKind::Raise | StatementKind::Throw => ControlFlowNodeRole::Statement,
            _ => ControlFlowNodeRole::Statement,
        };
        insert_node(
            graph,
            cfg_node(
                semantic,
                statement.cfg_node_id.clone(),
                statement.span,
                role,
                statement.text.clone(),
                Some(format!("{:?}", statement.kind)),
                "normalized statement",
            ),
        );
    }

    for condition in model
        .conditions_by_statement
        .values()
        .chain(model.exception_conditions_by_statement.values())
    {
        insert_node(
            graph,
            cfg_node(
                semantic,
                condition.cfg_node_id.clone(),
                condition.span,
                ControlFlowNodeRole::Condition,
                format!("{:?}", condition.kind),
                Some(format!("{:?}", condition.kind)),
                "structured condition",
            ),
        );
    }

    for control in model.expression_controls_by_statement.values().flatten() {
        insert_node(
            graph,
            cfg_node(
                semantic,
                control.cfg_node_id.clone(),
                control.span,
                ControlFlowNodeRole::Condition,
                control.label.clone(),
                Some(format!("{:?}", control.kind)),
                "expression control region",
            ),
        );
    }

    for return_fact in semantic
        .returns()
        .iter()
        .filter(|return_fact| return_fact.owner_id == semantic.owner_id())
    {
        let node_id = return_node_id(&semantic.callable().callable_id, return_fact);
        insert_node(
            graph,
            cfg_node(
                semantic,
                node_id,
                return_fact.source_span,
                ControlFlowNodeRole::Return,
                return_fact
                    .value
                    .clone()
                    .unwrap_or_else(|| "return".to_string()),
                None,
                "normalized return",
            ),
        );
    }

    for raise in semantic
        .raises()
        .iter()
        .filter(|raise| raise.owner_id == semantic.owner_id())
    {
        let node_id = raise_node_id(&semantic.callable().callable_id, raise);
        insert_node(
            graph,
            cfg_node(
                semantic,
                node_id,
                raise.source_span,
                ControlFlowNodeRole::Raise,
                raise.value.clone().unwrap_or_else(|| raise.text.clone()),
                None,
                "normalized raise/throw",
            ),
        );
    }
}

fn insert_basic_block_nodes(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    model: &CfgModel,
) {
    let mut ordinal = 0usize;
    for (parent_id, statement_ids) in &model.children_by_parent {
        let mut block = Vec::<NodeId>::new();
        for statement_id in statement_ids {
            let Some(statement) = model.statements.get(statement_id) else {
                continue;
            };
            if statement_starts_basic_block_boundary(statement.kind) {
                insert_basic_block_node(
                    graph,
                    semantic,
                    model,
                    parent_id.as_deref(),
                    ordinal,
                    &block,
                );
                if !block.is_empty() {
                    ordinal += 1;
                    block.clear();
                }
                continue;
            }
            block.push(statement.statement_id.clone());
        }
        insert_basic_block_node(
            graph,
            semantic,
            model,
            parent_id.as_deref(),
            ordinal,
            &block,
        );
        if !block.is_empty() {
            ordinal += 1;
        }
    }
}

fn insert_basic_block_node(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    model: &CfgModel,
    parent_id: Option<&str>,
    ordinal: usize,
    statement_ids: &[NodeId],
) {
    if statement_ids.is_empty() {
        return;
    }
    let first = &model.statements[&statement_ids[0]];
    let last = &model.statements[statement_ids.last().expect("non-empty basic block")];
    let node_id = stable_id(
        "basic-block",
        &[
            &semantic.callable().callable_id,
            parent_id.unwrap_or("top-level"),
            &ordinal.to_string(),
            &first.statement_id,
            &last.statement_id,
        ],
    );
    let span = SourceSpan {
        start_byte: first.span.start_byte,
        end_byte: last.span.end_byte,
        start_row: first.span.start_row,
        start_column: first.span.start_column,
        end_row: last.span.end_row,
        end_column: last.span.end_column,
    };
    insert_node(
        graph,
        graph_node(
            node_id.clone(),
            NodeKind::BasicBlock,
            node_owner(semantic),
            Some(span),
            Confidence::Exact,
            inference_evidence("compact straight-line CFG basic block"),
            NodeFact::BasicBlock(sg::BasicBlock {
                basic_block_id: node_id,
                callable_id: semantic.callable().callable_id.clone(),
                kind: BasicBlockKind::StraightLine,
                ordinal,
                statement_ids: statement_ids.to_vec(),
                entry_node_id: Some(first.cfg_node_id.clone()),
                exit_node_id: Some(last.cfg_node_id.clone()),
            }),
        ),
    );
}

fn statement_starts_basic_block_boundary(kind: StatementKind) -> bool {
    matches!(
        kind,
        StatementKind::Branch
            | StatementKind::Loop
            | StatementKind::Return
            | StatementKind::Raise
            | StatementKind::Throw
            | StatementKind::Break
            | StatementKind::Continue
            | StatementKind::Try
            | StatementKind::Catch
            | StatementKind::Finally
    )
}

fn connect_normal_exits(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    exits: &[OpenExit],
    target_id: &str,
) {
    for exit in exits.iter().filter(|exit| exit.kind.is_normal()) {
        add_cfg_edge_with_metadata(
            graph,
            semantic,
            &exit.node_id,
            target_id,
            exit.kind.flow_kind(),
            exit.kind.outcome(),
            exit.kind.branch_arm().cloned(),
            Some(exit.span),
        );
    }
}

fn non_normal_exits(exits: &[OpenExit]) -> Vec<OpenExit> {
    exits
        .iter()
        .filter(|exit| !exit.kind.is_normal())
        .cloned()
        .collect()
}

fn terminal_causes_from_exits(model: &CfgModel, exits: &[OpenExit]) -> Vec<TerminalCause> {
    let mut causes = exits
        .iter()
        .filter(|exit| !exit.kind.is_normal())
        .filter_map(|exit| terminal_statement_for_exit(model, exit))
        .collect::<Vec<_>>();
    causes.sort_by(|left, right| {
        (
            left.span.start_byte,
            left.span.end_byte,
            left.statement_id.as_str(),
        )
            .cmp(&(
                right.span.start_byte,
                right.span.end_byte,
                right.statement_id.as_str(),
            ))
    });
    causes.dedup_by(|left, right| left.statement_id == right.statement_id);
    causes
}

fn terminal_statement_for_exit(model: &CfgModel, exit: &OpenExit) -> Option<TerminalCause> {
    model
        .statements
        .values()
        .find(|statement| {
            statement.span == exit.span
                && matches!(
                    (statement.kind, &exit.kind),
                    (StatementKind::Return, OpenExitKind::Return)
                        | (StatementKind::Raise, OpenExitKind::Raise)
                        | (StatementKind::Throw, OpenExitKind::Raise)
                        | (StatementKind::Break, OpenExitKind::Break)
                        | (StatementKind::Continue, OpenExitKind::Continue)
                )
        })
        .map(TerminalCause::from_statement)
}

fn terminal_cause_summary(causes: &[TerminalCause]) -> String {
    match causes {
        [] => "a preceding terminal statement".to_string(),
        [cause] => format!("{} `{}`", cause.kind_label(), cause.text),
        _ => {
            let labels = causes
                .iter()
                .map(|cause| format!("{} `{}`", cause.kind_label(), cause.text))
                .collect::<Vec<_>>()
                .join(", ");
            format!("all incoming paths terminate at {labels}")
        }
    }
}

fn expression_is_control_region(expression: &sg::Expression) -> bool {
    match expression.kind {
        ExpressionKind::Conditional => true,
        ExpressionKind::BinaryOperator => expression
            .normalized
            .operator
            .as_deref()
            .is_some_and(|operator| matches!(operator, "&&" | "||" | "??" | "and" | "or")),
        _ => false,
    }
}

fn expression_control_node_id(callable_id: &str, expression: &sg::Expression) -> NodeId {
    stable_id(
        "cfg-node",
        &[callable_id, "expression-control", &expression.expression_id],
    )
}

fn structured_condition_node_id(
    callable_id: &str,
    condition: &sg::Condition,
    span: SourceSpan,
) -> NodeId {
    stable_id(
        "cfg-node",
        &[
            callable_id,
            "condition",
            &format!("{:?}", condition.kind),
            &span_key(span),
            &condition.condition_id,
        ],
    )
}

fn merge_node_id(callable_id: &str, kind: &str, anchor_id: &str) -> NodeId {
    stable_id("cfg-node", &[callable_id, "merge", kind, anchor_id])
}

fn branch_region_outcome(kind: ControlRegionKind) -> sg::ControlFlowOutcome {
    match kind {
        ControlRegionKind::BranchBody | ControlRegionKind::LoopBody => sg::ControlFlowOutcome::True,
        ControlRegionKind::ElseBody => sg::ControlFlowOutcome::False,
        ControlRegionKind::CatchBody => sg::ControlFlowOutcome::Exception,
        ControlRegionKind::FinallyBody => sg::ControlFlowOutcome::Finally,
        ControlRegionKind::TryBody
        | ControlRegionKind::LoopContinuation
        | ControlRegionKind::Unknown => sg::ControlFlowOutcome::Arm,
    }
}

fn branch_arm(region: &sg::ControlRegion, ordinal: usize) -> sg::ControlFlowBranchArm {
    sg::ControlFlowBranchArm {
        label: region.label.clone(),
        ordinal,
        region_kind: Some(region.kind),
    }
}

fn mark_normal_exits(
    exits: Vec<OpenExit>,
    outcome: sg::ControlFlowOutcome,
    branch_arm: Option<sg::ControlFlowBranchArm>,
) -> Vec<OpenExit> {
    exits
        .into_iter()
        .map(|exit| match exit.kind {
            OpenExitKind::Normal { flow_kind, .. } => OpenExit::normal_with_metadata(
                exit.node_id,
                flow_kind,
                outcome,
                branch_arm.clone(),
                exit.span,
            ),
            _ => exit,
        })
        .collect()
}

fn statement_sort_key(statement: &StatementInfo) -> (usize, usize, usize, StatementKind, NodeId) {
    (
        statement.span.start_byte,
        statement.span.end_byte,
        statement.ordinal,
        statement.kind,
        statement.statement_id.clone(),
    )
}

#[derive(Debug, Clone)]
struct StatementInfo {
    statement_id: NodeId,
    cfg_node_id: NodeId,
    parent_statement_id: Option<NodeId>,
    kind: StatementKind,
    ordinal: usize,
    text: String,
    span: SourceSpan,
}

#[derive(Debug, Clone)]
struct TerminalCause {
    statement_id: NodeId,
    cfg_node_id: NodeId,
    kind: StatementKind,
    text: String,
    span: SourceSpan,
}

impl TerminalCause {
    fn from_statement(statement: &StatementInfo) -> Self {
        Self {
            statement_id: statement.statement_id.clone(),
            cfg_node_id: statement.cfg_node_id.clone(),
            kind: statement.kind,
            text: statement.text.clone(),
            span: statement.span,
        }
    }

    fn kind_label(&self) -> &'static str {
        match self.kind {
            StatementKind::Return => "return",
            StatementKind::Raise => "raise",
            StatementKind::Throw => "throw",
            StatementKind::Break => "break",
            StatementKind::Continue => "continue",
            _ => "terminal statement",
        }
    }
}

#[derive(Debug, Clone)]
struct ConditionInfo {
    cfg_node_id: NodeId,
    kind: sg::ConditionKind,
    regions: Vec<sg::ControlRegion>,
    fallthrough: FallthroughBehavior,
    span: SourceSpan,
}

#[derive(Debug, Clone)]
struct ExpressionControlInfo {
    expression_id: NodeId,
    cfg_node_id: NodeId,
    kind: ExpressionKind,
    label: String,
    span: SourceSpan,
}

#[derive(Debug, Clone, Default)]
struct LoweringEnv {
    loop_condition_id: Option<NodeId>,
    exception_handler: Option<ExceptionHandler>,
}

#[derive(Debug, Clone)]
struct ExceptionHandler {
    node_id: NodeId,
    handles_exception: bool,
}

impl ExceptionHandler {
    fn catch(node_id: NodeId) -> Self {
        Self {
            node_id,
            handles_exception: true,
        }
    }

    fn finally(node_id: NodeId) -> Self {
        Self {
            node_id,
            handles_exception: false,
        }
    }
}

#[derive(Debug, Clone)]
struct LoweringResult {
    entry: Option<NodeId>,
    exits: Vec<OpenExit>,
    terminal_causes: Vec<TerminalCause>,
}

#[derive(Debug, Clone)]
struct OpenExit {
    node_id: NodeId,
    span: SourceSpan,
    kind: OpenExitKind,
}

impl OpenExit {
    fn normal(node_id: NodeId, flow_kind: ControlFlowKind, span: SourceSpan) -> Self {
        Self::normal_with_metadata(
            node_id,
            flow_kind,
            default_outcome_for_flow_kind(flow_kind),
            None,
            span,
        )
    }

    fn normal_with_metadata(
        node_id: NodeId,
        flow_kind: ControlFlowKind,
        outcome: sg::ControlFlowOutcome,
        branch_arm: Option<sg::ControlFlowBranchArm>,
        span: SourceSpan,
    ) -> Self {
        Self {
            node_id,
            span,
            kind: OpenExitKind::Normal {
                flow_kind,
                outcome,
                branch_arm,
            },
        }
    }
}

#[derive(Debug, Clone)]
enum OpenExitKind {
    Normal {
        flow_kind: ControlFlowKind,
        outcome: sg::ControlFlowOutcome,
        branch_arm: Option<sg::ControlFlowBranchArm>,
    },
    Break,
    Continue,
    Return,
    Raise,
}

impl OpenExitKind {
    fn is_normal(&self) -> bool {
        matches!(self, Self::Normal { .. })
    }

    fn flow_kind(&self) -> ControlFlowKind {
        match self {
            Self::Normal { flow_kind, .. } => *flow_kind,
            Self::Break | Self::Continue | Self::Return | Self::Raise => {
                ControlFlowKind::Sequential
            }
        }
    }

    fn outcome(&self) -> sg::ControlFlowOutcome {
        match self {
            Self::Normal { outcome, .. } => *outcome,
            Self::Break => sg::ControlFlowOutcome::Break,
            Self::Continue => sg::ControlFlowOutcome::Continue,
            Self::Return => sg::ControlFlowOutcome::Return,
            Self::Raise => sg::ControlFlowOutcome::Exception,
        }
    }

    fn branch_arm(&self) -> Option<&sg::ControlFlowBranchArm> {
        match self {
            Self::Normal { branch_arm, .. } => branch_arm.as_ref(),
            _ => None,
        }
    }
}

impl OpenExit {
    fn target_exit_id<'a>(&self, normal_exit_id: &'a str, exceptional_exit_id: &'a str) -> &'a str {
        match self.kind {
            OpenExitKind::Raise => exceptional_exit_id,
            OpenExitKind::Normal { .. }
            | OpenExitKind::Break
            | OpenExitKind::Continue
            | OpenExitKind::Return => normal_exit_id,
        }
    }
}

fn exits_after_finally(
    pre_finally_exits: Vec<OpenExit>,
    finally_exits: Vec<OpenExit>,
) -> Vec<OpenExit> {
    if pre_finally_exits.is_empty() {
        return finally_exits;
    }

    let mut exits = Vec::new();
    for pending in &pre_finally_exits {
        for finally_exit in &finally_exits {
            match finally_exit.kind {
                OpenExitKind::Normal { .. } => exits.push(OpenExit {
                    node_id: finally_exit.node_id.clone(),
                    span: finally_exit.span,
                    kind: pending.kind.clone(),
                }),
                _ => exits.push(finally_exit.clone()),
            }
        }
    }
    exits
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::analysis::source_graph::{build_python_supergraph, build_typescript_supergraph};
    use crate::ast::{
        CallAst, ConditionAst, ConditionKind as AstConditionKind, ExpressionAst,
        ExpressionKind as AstExpressionKind, FileAst, ProjectAst, RaiseAst, ReturnAst, SourceSpan,
        StatementAst, StatementKind as AstStatementKind,
    };
    use crate::supergraph::{
        ControlFlowKind, ControlFlowOutcome, DiagnosticKind, EdgeFact, NodeFact, ProgramSupergraph,
    };

    #[test]
    fn sg050_lowers_python_cfg_from_structured_statement_regions() {
        let graph = build_python_supergraph(&project("sample.py", "sample:<module>", true));

        assert_structured_cfg(&graph, true);
    }

    #[test]
    fn sg050_lowers_typescript_cfg_from_structured_statement_regions() {
        let graph = build_typescript_supergraph(&project("sample.ts", "sample:<module>", false));

        assert_structured_cfg(&graph, false);
    }

    #[test]
    fn sg051_routes_python_returns_and_unhandled_raises_to_separate_exits() {
        let graph =
            build_python_supergraph(&sg051_exit_project("sample.py", "sample:<module>", true));

        assert_callable_exit_cfg(&graph, true);
    }

    #[test]
    fn sg051_routes_typescript_returns_and_unhandled_throws_to_separate_exits() {
        let graph =
            build_typescript_supergraph(&sg051_exit_project("sample.ts", "sample:<module>", false));

        assert_callable_exit_cfg(&graph, false);
    }

    #[test]
    fn sg051_preserves_python_unhandled_exceptional_exit_through_finally() {
        let graph =
            build_python_supergraph(&sg051_finally_project("sample.py", "sample:<module>", true));

        assert_unhandled_finally_cfg(&graph);
    }

    #[test]
    fn sg051_preserves_typescript_unhandled_exceptional_exit_through_finally() {
        let graph = build_typescript_supergraph(&sg051_finally_project(
            "sample.ts",
            "sample:<module>",
            false,
        ));

        assert_unhandled_finally_cfg(&graph);
    }

    #[test]
    fn sg052_adds_python_merge_and_basic_block_nodes() {
        let graph = build_python_supergraph(&project("sample.py", "sample:<module>", true));

        assert_structured_cfg(&graph, true);
    }

    #[test]
    fn sg052_adds_typescript_merge_and_basic_block_nodes() {
        let graph = build_typescript_supergraph(&project("sample.ts", "sample:<module>", false));

        assert_structured_cfg(&graph, false);
    }

    #[test]
    fn sg053_adds_python_control_flow_outcome_metadata() {
        let graph = build_python_supergraph(&project("sample.py", "sample:<module>", true));

        assert_sg053_outcomes(&graph, true);
    }

    #[test]
    fn sg053_adds_typescript_control_flow_outcome_metadata() {
        let graph = build_typescript_supergraph(&project("sample.ts", "sample:<module>", false));

        assert_sg053_outcomes(&graph, false);
    }

    #[test]
    fn sg054_reports_python_unreachable_statements_with_terminal_explanations() {
        let graph = build_python_supergraph(&sg054_unreachable_project(
            "sample.py",
            "sample:<module>",
            true,
        ));

        assert_sg054_unreachable_diagnostics(&graph, true);
    }

    #[test]
    fn sg054_reports_typescript_unreachable_statements_with_terminal_explanations() {
        let graph = build_typescript_supergraph(&sg054_unreachable_project(
            "sample.ts",
            "sample:<module>",
            false,
        ));

        assert_sg054_unreachable_diagnostics(&graph, false);
    }

    #[test]
    fn sg121_asserts_python_cfg_semantics_with_exact_edges_and_metadata() {
        let graph = build_python_supergraph(&project("sample.py", "sample:<module>", true));

        assert_sg121_exact_cfg_edges(&graph, true);
        assert_sg054_unreachable_diagnostics(
            &build_python_supergraph(&sg054_unreachable_project(
                "sample.py",
                "sample:<module>",
                true,
            )),
            true,
        );
    }

    #[test]
    fn sg121_asserts_typescript_cfg_semantics_with_exact_edges_and_metadata() {
        let graph = build_typescript_supergraph(&project("sample.ts", "sample:<module>", false));

        assert_sg121_exact_cfg_edges(&graph, false);
        assert_sg054_unreachable_diagnostics(
            &build_typescript_supergraph(&sg054_unreachable_project(
                "sample.ts",
                "sample:<module>",
                false,
            )),
            false,
        );
    }

    #[test]
    fn sg121_asserts_exceptional_exit_semantics_for_unhandled_raise_and_throw() {
        let python =
            build_python_supergraph(&sg051_exit_project("sample.py", "sample:<module>", true));
        let typescript =
            build_typescript_supergraph(&sg051_exit_project("sample.ts", "sample:<module>", false));

        assert_sg121_exceptional_exit_metadata(&python, true);
        assert_sg121_exceptional_exit_metadata(&typescript, false);
    }

    #[test]
    fn sg121_documents_switch_match_fallback_until_parser_arm_facts_exist() {
        let python = build_python_supergraph(&sg121_switch_match_fallback_project(
            "sample.py",
            "sample:<module>",
            true,
        ));
        let typescript = build_typescript_supergraph(&sg121_switch_match_fallback_project(
            "sample.ts",
            "sample:<module>",
            false,
        ));

        assert_sg121_switch_match_fallback(&python, true);
        assert_sg121_switch_match_fallback(&typescript, false);
    }

    fn assert_structured_cfg(graph: &ProgramSupergraph, python: bool) {
        let edges = cfg_edges(graph);
        let entry = super::entry_node_id("sample:<module>");
        let start = cfg_statement_with_label(graph, "start()");
        let if_stmt = cfg_statement_with_label(
            graph,
            if python {
                "if flag:\n    success()\nelse:\n    recover()"
            } else {
                "if (flag) {\n  success();\n} else {\n  recover();\n}"
            },
        );
        let if_condition = cfg_condition_at(graph, span(15, 19));
        let if_merge = cfg_merge_at(graph, span(10, 80), "BranchMerge");
        let success = cfg_statement_with_label(graph, "success()");
        let recover = cfg_statement_with_label(graph, "recover()");
        let after_if = cfg_statement_with_label(graph, "after_if()");
        let loop_stmt = cfg_statement_with_label(
            graph,
            if python {
                "while keep:\n    tick()\n    if stop:\n        break\n    continue"
            } else {
                "while (keep) {\n  tick();\n  if (stop) {\n    break;\n  }\n  continue;\n}"
            },
        );
        let loop_condition = cfg_condition_at(graph, span(100, 104));
        let tick = cfg_statement_with_label(graph, "tick()");
        let stop_if = cfg_statement_with_label(
            graph,
            if python {
                "if stop:\n    break"
            } else {
                "if (stop) {\n    break;\n  }"
            },
        );
        let stop_condition = cfg_condition_at(graph, span(126, 130));
        let stop_merge = cfg_merge_at(graph, span(122, 152), "BranchMerge");
        let break_stmt = cfg_statement_with_label(graph, "break");
        let continue_stmt = cfg_statement_with_label(graph, "continue");
        let loop_merge = cfg_merge_at(graph, span(94, 180), "LoopExitMerge");
        let after_loop = cfg_statement_with_label(graph, "after_loop()");
        let try_stmt = cfg_statement_with_label(
            graph,
            if python {
                "try:\n    risky()\n    raise Error()\nexcept Error:\n    handled()\nfinally:\n    cleanup()"
            } else {
                "try {\n  risky();\n  throw error;\n} catch (error) {\n  handled();\n} finally {\n  cleanup();\n}"
            },
        );
        let try_condition = cfg_condition_at(graph, span(200, 290));
        let finally_entry_merge = cfg_merge_at(graph, span(200, 290), "FinallyEntryMerge");
        let exception_merge = cfg_merge_at(graph, span(200, 290), "ExceptionMerge");
        let risky = cfg_statement_with_label(graph, "risky()");
        let handled = cfg_statement_with_label(graph, "handled()");
        let finally_stmt = cfg_statement_with_label(graph, "finally");
        let cleanup = cfg_statement_with_label(graph, "cleanup()");
        let return_stmt = cfg_statement_with_label(graph, "return done");
        let exit = super::exit_node_id("sample:<module>");

        assert_edge(&edges, &entry, &start, ControlFlowKind::Entry);
        assert_edge(&edges, &start, &if_stmt, ControlFlowKind::Sequential);
        assert_edge(&edges, &if_stmt, &if_condition, ControlFlowKind::Sequential);
        assert_edge(&edges, &if_condition, &success, ControlFlowKind::Branch);
        assert_edge(&edges, &if_condition, &recover, ControlFlowKind::Branch);
        assert_edge(&edges, &success, &if_merge, ControlFlowKind::Sequential);
        assert_edge(&edges, &recover, &if_merge, ControlFlowKind::Sequential);
        assert_edge(&edges, &if_merge, &after_if, ControlFlowKind::Sequential);
        assert_edge(&edges, &after_if, &loop_stmt, ControlFlowKind::Sequential);
        assert_edge(
            &edges,
            &loop_stmt,
            &loop_condition,
            ControlFlowKind::Sequential,
        );
        assert_edge(&edges, &loop_condition, &tick, ControlFlowKind::Branch);
        assert_edge(&edges, &tick, &stop_if, ControlFlowKind::Sequential);
        assert_edge(
            &edges,
            &stop_if,
            &stop_condition,
            ControlFlowKind::Sequential,
        );
        assert_edge(
            &edges,
            &stop_condition,
            &break_stmt,
            ControlFlowKind::Branch,
        );
        assert_edge(
            &edges,
            &stop_condition,
            &stop_merge,
            ControlFlowKind::Branch,
        );
        assert_edge(
            &edges,
            &stop_merge,
            &continue_stmt,
            ControlFlowKind::Sequential,
        );
        assert_edge(
            &edges,
            &continue_stmt,
            &loop_condition,
            ControlFlowKind::LoopBack,
        );
        assert_edge(&edges, &break_stmt, &loop_merge, ControlFlowKind::Branch);
        assert_edge(
            &edges,
            &loop_condition,
            &loop_merge,
            ControlFlowKind::Branch,
        );
        assert_edge(
            &edges,
            &loop_merge,
            &after_loop,
            ControlFlowKind::Sequential,
        );
        assert_edge(&edges, &after_loop, &try_stmt, ControlFlowKind::Sequential);
        assert_edge(
            &edges,
            &try_stmt,
            &try_condition,
            ControlFlowKind::Sequential,
        );
        assert_edge(&edges, &try_condition, &risky, ControlFlowKind::Branch);
        assert_edge(
            &edges,
            &handled,
            &finally_entry_merge,
            ControlFlowKind::Branch,
        );
        assert_edge(
            &edges,
            &finally_entry_merge,
            &finally_stmt,
            ControlFlowKind::Sequential,
        );
        assert_edge(&edges, &finally_stmt, &cleanup, ControlFlowKind::Sequential);
        assert_edge(
            &edges,
            &cleanup,
            &exception_merge,
            ControlFlowKind::Sequential,
        );
        assert_edge(
            &edges,
            &exception_merge,
            &return_stmt,
            ControlFlowKind::Sequential,
        );
        assert_edge(&edges, &return_node(graph), &exit, ControlFlowKind::Exit);
        assert_basic_blocks(graph);

        assert!(
            !edges.iter().any(|(source, target, kind)| {
                source == &return_node(graph)
                    && target == &cfg_statement_with_label(graph, "unreachable_after_return()")
                    && *kind == ControlFlowKind::Sequential
            }),
            "return must not source-order fall through to the next statement"
        );
        assert!(
            graph.nodes.iter().any(|node| matches!(
                &node.fact,
                NodeFact::ControlFlow(control)
                    if control.semantic_kind.as_deref() == Some("BinaryOperator")
                        && control.label.contains(if python { "ready and enabled" } else { "ready && enabled" })
            )),
            "short-circuit expression should be represented as a CFG control node"
        );
    }

    fn assert_sg053_outcomes(graph: &ProgramSupergraph, python: bool) {
        let start = cfg_statement_with_label(graph, "start()");
        let if_condition = cfg_condition_at(graph, span(15, 19));
        let if_merge = cfg_merge_at(graph, span(10, 80), "BranchMerge");
        let success = cfg_statement_with_label(graph, "success()");
        let recover = cfg_statement_with_label(graph, "recover()");
        let after_if = cfg_statement_with_label(graph, "after_if()");
        let loop_condition = cfg_condition_at(graph, span(100, 104));
        let tick = cfg_statement_with_label(graph, "tick()");
        let stop_condition = cfg_condition_at(graph, span(126, 130));
        let stop_merge = cfg_merge_at(graph, span(122, 152), "BranchMerge");
        let break_stmt = cfg_statement_with_label(graph, "break");
        let continue_stmt = cfg_statement_with_label(graph, "continue");
        let loop_merge = cfg_merge_at(graph, span(94, 180), "LoopExitMerge");
        let after_loop = cfg_statement_with_label(graph, "after_loop()");
        let try_condition = cfg_condition_at(graph, span(200, 290));
        let finally_entry_merge = cfg_merge_at(graph, span(200, 290), "FinallyEntryMerge");
        let exception_merge = cfg_merge_at(graph, span(200, 290), "ExceptionMerge");
        let risky = cfg_statement_with_label(graph, "risky()");
        let handled_raise = cfg_raise_at(graph, span(224, 236));
        let catch_stmt = cfg_statement_with_label(graph, "catch");
        let handled = cfg_statement_with_label(graph, "handled()");
        let finally_stmt = cfg_statement_with_label(graph, "finally");
        let cleanup = cfg_statement_with_label(graph, "cleanup()");
        let return_cfg = return_node(graph);
        let normal_exit = super::normal_exit_node_id("sample:<module>");

        assert_edge_outcome(
            graph,
            &start,
            &cfg_statement_with_label(
                graph,
                if python {
                    "if flag:\n    success()\nelse:\n    recover()"
                } else {
                    "if (flag) {\n  success();\n} else {\n  recover();\n}"
                },
            ),
            ControlFlowOutcome::Fallthrough,
            None,
        );
        assert_edge_outcome(
            graph,
            &if_condition,
            &success,
            ControlFlowOutcome::True,
            Some("branch-body"),
        );
        assert_edge_outcome(
            graph,
            &if_condition,
            &recover,
            ControlFlowOutcome::False,
            Some("else-body"),
        );
        assert_edge_outcome(
            graph,
            &success,
            &if_merge,
            ControlFlowOutcome::True,
            Some("branch-body"),
        );
        assert_edge_outcome(
            graph,
            &recover,
            &if_merge,
            ControlFlowOutcome::False,
            Some("else-body"),
        );
        assert_edge_outcome(
            graph,
            &if_merge,
            &after_if,
            ControlFlowOutcome::Fallthrough,
            None,
        );
        assert_edge_outcome(
            graph,
            &loop_condition,
            &tick,
            ControlFlowOutcome::True,
            Some("loop-body"),
        );
        assert_edge_outcome(
            graph,
            &stop_condition,
            &break_stmt,
            ControlFlowOutcome::True,
            Some("branch-body"),
        );
        assert_edge_outcome(
            graph,
            &stop_condition,
            &stop_merge,
            ControlFlowOutcome::False,
            None,
        );
        assert_edge_outcome(
            graph,
            &break_stmt,
            &loop_merge,
            ControlFlowOutcome::Break,
            None,
        );
        assert_edge_outcome(
            graph,
            &continue_stmt,
            &loop_condition,
            ControlFlowOutcome::Continue,
            None,
        );
        assert_edge_outcome(
            graph,
            &loop_condition,
            &loop_merge,
            ControlFlowOutcome::False,
            None,
        );
        assert_edge_outcome(
            graph,
            &loop_merge,
            &after_loop,
            ControlFlowOutcome::Fallthrough,
            None,
        );
        assert_edge_outcome(
            graph,
            &try_condition,
            &risky,
            ControlFlowOutcome::Fallthrough,
            Some("try-body"),
        );
        assert_edge_outcome(
            graph,
            &handled_raise,
            &catch_stmt,
            ControlFlowOutcome::Exception,
            None,
        );
        assert_edge_outcome(
            graph,
            &handled,
            &finally_entry_merge,
            ControlFlowOutcome::Finally,
            Some("finally-body"),
        );
        assert_edge_outcome(
            graph,
            &finally_entry_merge,
            &finally_stmt,
            ControlFlowOutcome::Finally,
            Some("finally-body"),
        );
        assert_edge_outcome(
            graph,
            &cleanup,
            &exception_merge,
            ControlFlowOutcome::Exception,
            Some("catch-body"),
        );
        assert_edge_outcome(
            graph,
            &return_cfg,
            &normal_exit,
            ControlFlowOutcome::Return,
            None,
        );
        assert!(
            cfg_control_flow_facts(graph)
                .iter()
                .all(|(_, _, _, flow)| flow.outcome != ControlFlowOutcome::Unknown),
            "all CFG successors should carry an explicit outcome"
        );
    }

    fn assert_sg054_unreachable_diagnostics(graph: &ProgramSupergraph, python: bool) {
        let expected_unreachable = [
            "after_break()",
            "after_continue()",
            "after_raise()",
            "after_return()",
        ];
        for label in expected_unreachable {
            assert_unreachable_diagnostic_for(graph, label);
        }

        for label in [
            "alive()",
            "after_branch()",
            "after_loop()",
            "handled()",
            "cleanup()",
            "after_try()",
        ] {
            assert_no_unreachable_diagnostic_for(graph, label);
        }

        let return_statement = cfg_statement_with_label(graph, "return final");
        let after_return = cfg_statement_with_label(graph, "after_return()");
        let raise_statement = cfg_statement_with_label(
            graph,
            if python {
                "raise Error()"
            } else {
                "throw error"
            },
        );
        let after_raise = cfg_statement_with_label(graph, "after_raise()");
        let break_statement = cfg_statement_with_label(graph, "break");
        let after_break = cfg_statement_with_label(graph, "after_break()");
        let continue_statement = cfg_statement_with_label(graph, "continue");
        let after_continue = cfg_statement_with_label(graph, "after_continue()");

        let edges = cfg_edges(graph);
        for (source, target) in [
            (&return_statement, &after_return),
            (&raise_statement, &after_raise),
            (&break_statement, &after_break),
            (&continue_statement, &after_continue),
        ] {
            assert!(
                !edges
                    .iter()
                    .any(|(edge_source, edge_target, kind)| edge_source == source
                        && edge_target == target
                        && *kind == ControlFlowKind::Sequential),
                "terminal statement {source} must not normally flow to unreachable {target}"
            );
        }

        let after_branch = cfg_statement_with_label(graph, "after_branch()");
        assert!(
            edges
                .iter()
                .any(|(_, edge_target, kind)| edge_target == &after_branch
                    && *kind == ControlFlowKind::Sequential),
            "alternate branch fallthrough should keep after_branch reachable"
        );

        let catch_stmt = cfg_statement_with_label(graph, "catch");
        let handled = cfg_statement_with_label(graph, "handled()");
        assert_edge(&edges, &catch_stmt, &handled, ControlFlowKind::Sequential);
    }

    fn assert_sg121_exact_cfg_edges(graph: &ProgramSupergraph, python: bool) {
        let entry = super::entry_node_id("sample:<module>");
        let normal_exit = super::normal_exit_node_id("sample:<module>");
        let start = cfg_statement_with_label(graph, "start()");
        let if_stmt = cfg_statement_with_label(
            graph,
            if python {
                "if flag:\n    success()\nelse:\n    recover()"
            } else {
                "if (flag) {\n  success();\n} else {\n  recover();\n}"
            },
        );
        let if_condition = cfg_condition_at(graph, span(15, 19));
        let success = cfg_statement_with_label(graph, "success()");
        let recover = cfg_statement_with_label(graph, "recover()");
        let if_merge = cfg_merge_at(graph, span(10, 80), "BranchMerge");
        let after_if = cfg_statement_with_label(graph, "after_if()");
        let loop_stmt = cfg_statement_with_label(
            graph,
            if python {
                "while keep:\n    tick()\n    if stop:\n        break\n    continue"
            } else {
                "while (keep) {\n  tick();\n  if (stop) {\n    break;\n  }\n  continue;\n}"
            },
        );
        let loop_condition = cfg_condition_at(graph, span(100, 104));
        let tick = cfg_statement_with_label(graph, "tick()");
        let stop_if = cfg_statement_with_label(
            graph,
            if python {
                "if stop:\n    break"
            } else {
                "if (stop) {\n    break;\n  }"
            },
        );
        let stop_condition = cfg_condition_at(graph, span(126, 130));
        let break_stmt = cfg_statement_with_label(graph, "break");
        let stop_merge = cfg_merge_at(graph, span(122, 152), "BranchMerge");
        let continue_stmt = cfg_statement_with_label(graph, "continue");
        let loop_merge = cfg_merge_at(graph, span(94, 180), "LoopExitMerge");
        let after_loop = cfg_statement_with_label(graph, "after_loop()");
        let try_stmt = cfg_statement_with_label(
            graph,
            if python {
                "try:\n    risky()\n    raise Error()\nexcept Error:\n    handled()\nfinally:\n    cleanup()"
            } else {
                "try {\n  risky();\n  throw error;\n} catch (error) {\n  handled();\n} finally {\n  cleanup();\n}"
            },
        );
        let try_condition = cfg_condition_at(graph, span(200, 290));
        let risky = cfg_statement_with_label(graph, "risky()");
        let handled_raise_stmt = cfg_statement_with_label(
            graph,
            if python {
                "raise Error()"
            } else {
                "throw error"
            },
        );
        let handled_raise = cfg_raise_at(graph, span(224, 236));
        let catch_stmt = cfg_statement_with_label(graph, "catch");
        let handled = cfg_statement_with_label(graph, "handled()");
        let finally_entry_merge = cfg_merge_at(graph, span(200, 290), "FinallyEntryMerge");
        let finally_stmt = cfg_statement_with_label(graph, "finally");
        let cleanup = cfg_statement_with_label(graph, "cleanup()");
        let exception_merge = cfg_merge_at(graph, span(200, 290), "ExceptionMerge");
        let return_stmt = cfg_statement_with_label(graph, "return done");
        let return_cfg = cfg_return_at(graph, span(300, 311));
        let short_circuit = cfg_expression_control_with_label(
            graph,
            if python {
                "ready and enabled"
            } else {
                "ready && enabled"
            },
        );
        let short_circuit_statement = cfg_statement_with_label(
            graph,
            if python {
                "ready and enabled"
            } else {
                "ready && enabled"
            },
        );

        let expected = BTreeSet::from([
            cfg_edge(
                &entry,
                &start,
                ControlFlowKind::Entry,
                ControlFlowOutcome::Entry,
                None,
            ),
            cfg_edge(
                &start,
                &if_stmt,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
            cfg_edge(
                &if_stmt,
                &if_condition,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
            cfg_edge(
                &if_condition,
                &success,
                ControlFlowKind::Branch,
                ControlFlowOutcome::True,
                Some(("branch-body", 0)),
            ),
            cfg_edge(
                &if_condition,
                &recover,
                ControlFlowKind::Branch,
                ControlFlowOutcome::False,
                Some(("else-body", 1)),
            ),
            cfg_edge(
                &success,
                &if_merge,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::True,
                Some(("branch-body", 0)),
            ),
            cfg_edge(
                &recover,
                &if_merge,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::False,
                Some(("else-body", 1)),
            ),
            cfg_edge(
                &if_merge,
                &after_if,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
            cfg_edge(
                &after_if,
                &loop_stmt,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
            cfg_edge(
                &loop_stmt,
                &loop_condition,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
            cfg_edge(
                &loop_condition,
                &tick,
                ControlFlowKind::Branch,
                ControlFlowOutcome::True,
                Some(("loop-body", 0)),
            ),
            cfg_edge(
                &tick,
                &stop_if,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
            cfg_edge(
                &stop_if,
                &stop_condition,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
            cfg_edge(
                &stop_condition,
                &break_stmt,
                ControlFlowKind::Branch,
                ControlFlowOutcome::True,
                Some(("branch-body", 0)),
            ),
            cfg_edge(
                &stop_condition,
                &stop_merge,
                ControlFlowKind::Branch,
                ControlFlowOutcome::False,
                None,
            ),
            cfg_edge(
                &stop_merge,
                &continue_stmt,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
            cfg_edge(
                &continue_stmt,
                &loop_condition,
                ControlFlowKind::LoopBack,
                ControlFlowOutcome::Continue,
                None,
            ),
            cfg_edge(
                &break_stmt,
                &loop_merge,
                ControlFlowKind::Branch,
                ControlFlowOutcome::Break,
                None,
            ),
            cfg_edge(
                &loop_condition,
                &loop_merge,
                ControlFlowKind::Branch,
                ControlFlowOutcome::False,
                None,
            ),
            cfg_edge(
                &loop_merge,
                &after_loop,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
            cfg_edge(
                &after_loop,
                &try_stmt,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
            cfg_edge(
                &try_stmt,
                &try_condition,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
            cfg_edge(
                &try_condition,
                &risky,
                ControlFlowKind::Branch,
                ControlFlowOutcome::Fallthrough,
                Some(("try-body", 0)),
            ),
            cfg_edge(
                &risky,
                &handled_raise_stmt,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
            cfg_edge(
                &handled_raise_stmt,
                &handled_raise,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
            cfg_edge(
                &handled_raise,
                &catch_stmt,
                ControlFlowKind::Branch,
                ControlFlowOutcome::Exception,
                None,
            ),
            cfg_edge(
                &try_condition,
                &catch_stmt,
                ControlFlowKind::Branch,
                ControlFlowOutcome::Exception,
                Some(("catch-body", 1)),
            ),
            cfg_edge(
                &catch_stmt,
                &handled,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
            cfg_edge(
                &handled,
                &finally_entry_merge,
                ControlFlowKind::Branch,
                ControlFlowOutcome::Finally,
                Some(("finally-body", 2)),
            ),
            cfg_edge(
                &finally_entry_merge,
                &finally_stmt,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Finally,
                Some(("finally-body", 2)),
            ),
            cfg_edge(
                &finally_stmt,
                &cleanup,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
            cfg_edge(
                &cleanup,
                &exception_merge,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Exception,
                Some(("catch-body", 1)),
            ),
            cfg_edge(
                &exception_merge,
                &return_stmt,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
            cfg_edge(
                &return_stmt,
                &return_cfg,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
            cfg_edge(
                &return_cfg,
                &normal_exit,
                ControlFlowKind::Exit,
                ControlFlowOutcome::Return,
                None,
            ),
            cfg_edge(
                &short_circuit,
                &short_circuit_statement,
                ControlFlowKind::Branch,
                ControlFlowOutcome::Arm,
                None,
            ),
            cfg_edge(
                &short_circuit,
                &short_circuit_statement,
                ControlFlowKind::Sequential,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
        ]);

        assert_eq!(cfg_edge_facts(graph), expected);
        assert_cfg_node_role(graph, &entry, "Entry");
        assert_cfg_node_role(graph, &if_condition, "Condition");
        assert_cfg_node_role(graph, &if_merge, "Merge");
        assert_cfg_node_role(graph, &loop_merge, "Merge");
        assert_cfg_node_role(graph, &return_cfg, "Return");
        assert_basic_blocks(graph);
        assert!(
            cfg_control_flow_facts(graph)
                .iter()
                .all(|(_, _, _, flow)| flow.outcome != ControlFlowOutcome::Unknown),
            "SG-121 fixture should not emit unknown CFG outcomes"
        );
    }

    fn assert_sg121_exceptional_exit_metadata(graph: &ProgramSupergraph, python: bool) {
        let exceptional_exit = super::exceptional_exit_node_id("sample:<module>");
        let fatal_raise = cfg_raise_at(graph, span(25, 38));
        let catch_stmt = cfg_statement_with_label(graph, "catch");
        let handled_raise = cfg_raise_at(graph, span(94, 106));

        assert_exact_edge(
            graph,
            &fatal_raise,
            &exceptional_exit,
            ControlFlowKind::Exit,
            ControlFlowOutcome::Exception,
            None,
        );
        assert_exact_edge(
            graph,
            &handled_raise,
            &catch_stmt,
            ControlFlowKind::Branch,
            ControlFlowOutcome::Exception,
            None,
        );
        assert!(
            !cfg_edge_facts(graph).iter().any(|edge| {
                edge.source == handled_raise
                    && edge.target == catch_stmt
                    && edge.flow_kind == ControlFlowKind::Exit
            }),
            "handled {} should not be modeled as an immediate callable exit",
            if python { "raise" } else { "throw" }
        );
    }

    fn assert_sg121_switch_match_fallback(graph: &ProgramSupergraph, python: bool) {
        let entry = super::entry_node_id("sample:<module>");
        let normal_exit = super::normal_exit_node_id("sample:<module>");
        let statement = cfg_statement_with_label(
            graph,
            if python {
                "match status:\n    case 'ready':\n        ready()"
            } else {
                "switch (status) {\n  case 'ready':\n    ready();\n}"
            },
        );
        let expected = BTreeSet::from([
            cfg_edge(
                &entry,
                &statement,
                ControlFlowKind::Entry,
                ControlFlowOutcome::Entry,
                None,
            ),
            cfg_edge(
                &statement,
                &normal_exit,
                ControlFlowKind::Exit,
                ControlFlowOutcome::Fallthrough,
                None,
            ),
        ]);

        assert_eq!(cfg_edge_facts(graph), expected);
        assert_cfg_node_role(graph, &statement, "Statement");
        assert!(
            graph.nodes.iter().all(|node| {
                !matches!(
                    &node.fact,
                    NodeFact::ControlFlow(control)
                        if control.role == crate::supergraph::ControlFlowNodeRole::Condition
                            || control.role == crate::supergraph::ControlFlowNodeRole::Merge
                )
            }),
            "switch/match parser facts are currently Unknown statements, not arm-level CFG regions"
        );
        assert!(
            graph.nodes.iter().any(|node| matches!(
                &node.fact,
                NodeFact::ControlFlow(control)
                    if control.cfg_node_id == statement
                        && control.semantic_kind.as_deref() == Some("Unknown")
            )),
            "switch/match fallback should remain explicitly limited until parser arm facts exist"
        );
    }

    fn assert_callable_exit_cfg(graph: &ProgramSupergraph, python: bool) {
        let edges = cfg_edges(graph);
        let normal_exit = super::normal_exit_node_id("sample:<module>");
        let exceptional_exit = super::exceptional_exit_node_id("sample:<module>");
        let start = cfg_statement_with_label(graph, "start()");
        let fatal_if = cfg_statement_with_label(
            graph,
            if python {
                "if fatal:\n    raise Fatal()"
            } else {
                "if (fatal) {\n  throw fatal;\n}"
            },
        );
        let fatal_condition = cfg_condition_at(graph, span(13, 18));
        let fatal_merge = cfg_merge_at(graph, span(10, 45), "BranchMerge");
        let unhandled_raise_stmt = cfg_statement_with_label(
            graph,
            if python {
                "raise Fatal()"
            } else {
                "throw fatal"
            },
        );
        let unhandled_raise = cfg_raise_at(graph, span(25, 38));
        let after_if = cfg_statement_with_label(graph, "after_if()");
        let try_condition = cfg_condition_at(graph, span(70, 160));
        let finally_entry_merge = cfg_merge_at(graph, span(70, 160), "FinallyEntryMerge");
        let exception_merge = cfg_merge_at(graph, span(70, 160), "ExceptionMerge");
        let risky = cfg_statement_with_label(graph, "risky()");
        let handled_raise = cfg_raise_at(graph, span(94, 106));
        let catch_stmt = cfg_statement_with_label(graph, "catch");
        let handled = cfg_statement_with_label(graph, "handled()");
        let finally_stmt = cfg_statement_with_label(graph, "finally");
        let cleanup = cfg_statement_with_label(graph, "cleanup()");
        let return_stmt = cfg_statement_with_label(graph, "return done");
        let return_cfg = cfg_return_at(graph, span(170, 181));

        assert_edge(&edges, &start, &fatal_if, ControlFlowKind::Sequential);
        assert_edge(
            &edges,
            &fatal_if,
            &fatal_condition,
            ControlFlowKind::Sequential,
        );
        assert_edge(
            &edges,
            &fatal_condition,
            &unhandled_raise_stmt,
            ControlFlowKind::Branch,
        );
        assert_edge(
            &edges,
            &unhandled_raise_stmt,
            &unhandled_raise,
            ControlFlowKind::Sequential,
        );
        assert_edge(
            &edges,
            &fatal_condition,
            &fatal_merge,
            ControlFlowKind::Branch,
        );
        assert_edge(&edges, &fatal_merge, &after_if, ControlFlowKind::Sequential);
        assert_edge(
            &edges,
            &unhandled_raise,
            &exceptional_exit,
            ControlFlowKind::Exit,
        );
        assert_edge(&edges, &try_condition, &risky, ControlFlowKind::Branch);
        assert_edge(&edges, &handled_raise, &catch_stmt, ControlFlowKind::Branch);
        assert_edge(&edges, &catch_stmt, &handled, ControlFlowKind::Sequential);
        assert!(
            !edges.iter().any(|(source, target, kind)| {
                source == &handled_raise
                    && target == &exceptional_exit
                    && *kind == ControlFlowKind::Exit
            }),
            "handled raise/throw must flow to the handler, not directly to exceptional exit"
        );
        assert_edge(
            &edges,
            &handled,
            &finally_entry_merge,
            ControlFlowKind::Branch,
        );
        assert_edge(
            &edges,
            &finally_entry_merge,
            &finally_stmt,
            ControlFlowKind::Sequential,
        );
        assert_edge(&edges, &finally_stmt, &cleanup, ControlFlowKind::Sequential);
        assert_edge(
            &edges,
            &cleanup,
            &exception_merge,
            ControlFlowKind::Sequential,
        );
        assert_edge(
            &edges,
            &exception_merge,
            &return_stmt,
            ControlFlowKind::Sequential,
        );
        assert_edge(
            &edges,
            &return_stmt,
            &return_cfg,
            ControlFlowKind::Sequential,
        );
        assert_edge(&edges, &return_cfg, &normal_exit, ControlFlowKind::Exit);
    }

    fn assert_unhandled_finally_cfg(graph: &ProgramSupergraph) {
        let edges = cfg_edges(graph);
        let exceptional_exit = super::exceptional_exit_node_id("sample:<module>");
        let raise = cfg_raise_at(graph, span(12, 24));
        let finally_entry_merge = cfg_merge_at(graph, span(0, 80), "FinallyEntryMerge");
        let finally_stmt = cfg_statement_with_label(graph, "finally");
        let cleanup = cfg_statement_with_label(graph, "cleanup()");

        assert_edge(
            &edges,
            &raise,
            &finally_entry_merge,
            ControlFlowKind::Branch,
        );
        assert_edge(
            &edges,
            &finally_entry_merge,
            &finally_stmt,
            ControlFlowKind::Sequential,
        );
        assert_edge(&edges, &finally_stmt, &cleanup, ControlFlowKind::Sequential);
        assert_edge(&edges, &cleanup, &exceptional_exit, ControlFlowKind::Exit);
    }

    fn cfg_edges(graph: &ProgramSupergraph) -> BTreeSet<(String, String, ControlFlowKind)> {
        cfg_control_flow_facts(graph)
            .into_iter()
            .map(|(source, target, _, flow)| (source, target, flow.flow_kind))
            .collect()
    }

    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
    struct ExpectedCfgEdge {
        source: String,
        target: String,
        flow_kind: ControlFlowKind,
        outcome: ControlFlowOutcome,
        branch_arm: Option<(String, usize)>,
    }

    fn cfg_edge(
        source: &str,
        target: &str,
        flow_kind: ControlFlowKind,
        outcome: ControlFlowOutcome,
        branch_arm: Option<(&str, usize)>,
    ) -> ExpectedCfgEdge {
        ExpectedCfgEdge {
            source: source.to_string(),
            target: target.to_string(),
            flow_kind,
            outcome,
            branch_arm: branch_arm.map(|(label, ordinal)| (label.to_string(), ordinal)),
        }
    }

    fn cfg_edge_facts(graph: &ProgramSupergraph) -> BTreeSet<ExpectedCfgEdge> {
        cfg_control_flow_facts(graph)
            .into_iter()
            .map(|(source, target, _, flow)| ExpectedCfgEdge {
                source,
                target,
                flow_kind: flow.flow_kind,
                outcome: flow.outcome,
                branch_arm: flow.branch_arm.map(|arm| (arm.label, arm.ordinal)),
            })
            .collect()
    }

    fn assert_exact_edge(
        graph: &ProgramSupergraph,
        source: &str,
        target: &str,
        flow_kind: ControlFlowKind,
        outcome: ControlFlowOutcome,
        branch_arm: Option<(&str, usize)>,
    ) {
        let edge = cfg_edge(source, target, flow_kind, outcome, branch_arm);
        assert!(
            cfg_edge_facts(graph).contains(&edge),
            "missing exact CFG edge {edge:?}"
        );
    }

    fn assert_edge(
        edges: &BTreeSet<(String, String, ControlFlowKind)>,
        source: &str,
        target: &str,
        kind: ControlFlowKind,
    ) {
        assert!(
            edges.contains(&(source.to_string(), target.to_string(), kind)),
            "missing CFG edge {source} -> {target} ({kind:?})"
        );
    }

    fn assert_unreachable_diagnostic_for(graph: &ProgramSupergraph, label: &str) {
        let statement_id = statement_fact_with_text(graph, label);
        let diagnostic = graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Diagnostic(diagnostic)
                    if diagnostic.kind == DiagnosticKind::UnreachableStatement
                        && diagnostic.related.contains(&statement_id) =>
                {
                    Some(diagnostic)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing unreachable diagnostic for {label}"));
        assert!(
            diagnostic.message.contains("prevents normal fallthrough"),
            "unreachable diagnostic should explain terminal fallthrough blocker"
        );
        assert!(
            diagnostic.related.len() >= 4,
            "unreachable diagnostic should relate unreachable and terminal CFG/statement facts"
        );
    }

    fn assert_no_unreachable_diagnostic_for(graph: &ProgramSupergraph, label: &str) {
        let statement_id = statement_fact_with_text(graph, label);
        assert!(
            !graph.nodes.iter().any(|node| matches!(
                &node.fact,
                NodeFact::Diagnostic(diagnostic)
                    if diagnostic.kind == DiagnosticKind::UnreachableStatement
                        && diagnostic.related.contains(&statement_id)
            )),
            "reachable statement {label} should not receive an unreachable diagnostic"
        );
    }

    fn statement_fact_with_text(graph: &ProgramSupergraph, text: &str) -> String {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Statement(statement) => {
                    let cfg_node = cfg_statement_with_label(graph, text);
                    graph
                        .nodes
                        .iter()
                        .find(|candidate| {
                            candidate.node_id == cfg_node && candidate.span == node.span
                        })
                        .map(|_| statement.statement_id.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing statement fact {text}"))
    }

    fn cfg_control_flow_facts(
        graph: &ProgramSupergraph,
    ) -> Vec<(String, String, String, crate::supergraph::ControlFlow)> {
        graph
            .control_flow_view()
            .edges_for_callable("sample:<module>")
            .filter_map(|edge| match &edge.fact {
                EdgeFact::ControlFlow(flow) => Some((
                    edge.source_id.clone(),
                    edge.target_id.clone()?,
                    edge.edge_id.clone(),
                    flow.clone(),
                )),
                _ => None,
            })
            .collect()
    }

    fn assert_edge_outcome(
        graph: &ProgramSupergraph,
        source: &str,
        target: &str,
        outcome: ControlFlowOutcome,
        arm_label: Option<&str>,
    ) {
        let matching = cfg_control_flow_facts(graph)
            .into_iter()
            .filter(|(edge_source, edge_target, _, flow)| {
                edge_source == source && edge_target == target && flow.outcome == outcome
            })
            .collect::<Vec<_>>();
        assert!(
            !matching.is_empty(),
            "missing CFG edge {source} -> {target} with outcome {outcome:?}"
        );
        if let Some(arm_label) = arm_label {
            assert!(
                matching.iter().any(|(_, _, _, flow)| flow
                    .branch_arm
                    .as_ref()
                    .is_some_and(|arm| arm.label == arm_label)),
                "missing CFG edge {source} -> {target} arm label {arm_label}"
            );
        }
    }

    fn cfg_statement_with_label(graph: &ProgramSupergraph, label: &str) -> String {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::ControlFlow(control)
                    if control.label == label
                        && control.role == crate::supergraph::ControlFlowNodeRole::Statement =>
                {
                    Some(control.cfg_node_id.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing CFG statement {label}"))
    }

    fn cfg_condition_at(graph: &ProgramSupergraph, span: SourceSpan) -> String {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::ControlFlow(control)
                    if node.span == Some(span)
                        && control.role == crate::supergraph::ControlFlowNodeRole::Condition =>
                {
                    Some(control.cfg_node_id.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing CFG condition at {span:?}"))
    }

    fn cfg_expression_control_with_label(graph: &ProgramSupergraph, label: &str) -> String {
        graph
            .control_flow_view()
            .nodes_for_callable("sample:<module>")
            .find_map(|node| match &node.fact {
                NodeFact::ControlFlow(control)
                    if control.role == crate::supergraph::ControlFlowNodeRole::Condition
                        && control.semantic_kind.as_deref() == Some("BinaryOperator")
                        && control.label == label =>
                {
                    Some(control.cfg_node_id.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing expression-control CFG node {label}"))
    }

    fn cfg_merge_at(graph: &ProgramSupergraph, span: SourceSpan, semantic_kind: &str) -> String {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::ControlFlow(control)
                    if node.span == Some(span)
                        && control.role == crate::supergraph::ControlFlowNodeRole::Merge
                        && control.semantic_kind.as_deref() == Some(semantic_kind) =>
                {
                    Some(control.cfg_node_id.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing CFG merge {semantic_kind} at {span:?}"))
    }

    fn assert_cfg_node_role(graph: &ProgramSupergraph, cfg_node_id: &str, expected_role: &str) {
        let actual = graph
            .control_flow_view()
            .nodes_for_callable("sample:<module>")
            .find_map(|node| match &node.fact {
                NodeFact::ControlFlow(control) if control.cfg_node_id == cfg_node_id => {
                    Some(format!("{:?}", control.role))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing CFG node {cfg_node_id}"));
        assert_eq!(actual, expected_role);
    }

    fn assert_basic_blocks(graph: &ProgramSupergraph) {
        let blocks = graph
            .nodes
            .iter()
            .filter_map(|node| match &node.fact {
                NodeFact::BasicBlock(block) => Some((node, block)),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(!blocks.is_empty(), "expected compact BasicBlock facts");
        assert!(
            blocks.iter().all(|(node, block)| {
                node.kind == crate::supergraph::NodeKind::BasicBlock
                    && block.callable_id == "sample:<module>"
                    && !block.statement_ids.is_empty()
                    && block.entry_node_id.is_some()
                    && block.exit_node_id.is_some()
            }),
            "basic blocks should be deterministic non-empty straight-line regions"
        );
        assert!(
            graph
                .indexes
                .nodes_by_kind
                .get(&crate::supergraph::NodeKind::BasicBlock)
                .is_some_and(|ids| ids.len() == blocks.len()),
            "BasicBlock nodes should be indexed by node kind"
        );
    }

    fn return_node(graph: &ProgramSupergraph) -> String {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::ControlFlow(control)
                    if control.role == crate::supergraph::ControlFlowNodeRole::Return =>
                {
                    Some(control.cfg_node_id.clone())
                }
                _ => None,
            })
            .expect("return CFG node")
    }

    fn cfg_return_at(graph: &ProgramSupergraph, span: SourceSpan) -> String {
        cfg_node_at_role(graph, span, crate::supergraph::ControlFlowNodeRole::Return)
    }

    fn cfg_raise_at(graph: &ProgramSupergraph, span: SourceSpan) -> String {
        cfg_node_at_role(graph, span, crate::supergraph::ControlFlowNodeRole::Raise)
    }

    fn cfg_node_at_role(
        graph: &ProgramSupergraph,
        span: SourceSpan,
        role: crate::supergraph::ControlFlowNodeRole,
    ) -> String {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::ControlFlow(control)
                    if node.span == Some(span) && control.role == role =>
                {
                    Some(control.cfg_node_id.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing CFG node at {span:?} with role {role:?}"))
    }

    fn project(path: &str, owner_id: &str, python: bool) -> ProjectAst {
        ProjectAst {
            root: String::new(),
            files: vec![FileAst {
                path: path.to_string(),
                imports: Vec::new(),
                assignments: Vec::new(),
                calls: Vec::<CallAst>::new(),
                symbols: Vec::new(),
                statements: statements(owner_id, python),
                expressions: expressions(owner_id, python),
                conditions: conditions(owner_id),
                definitions: Vec::new(),
                uses: Vec::new(),
                returns: vec![ReturnAst {
                    value: Some("done".to_string()),
                    owner_id: owner_id.to_string(),
                    source_span: span(300, 311),
                }],
                raises: vec![RaiseAst {
                    text: if python {
                        "raise Error()".to_string()
                    } else {
                        "throw error".to_string()
                    },
                    value: Some(if python {
                        "Error()".to_string()
                    } else {
                        "error".to_string()
                    }),
                    owner_id: owner_id.to_string(),
                    source_span: span(224, 236),
                }],
                field_accesses: Vec::new(),
                index_accesses: Vec::new(),
            }],
        }
    }

    fn sg054_unreachable_project(path: &str, owner_id: &str, python: bool) -> ProjectAst {
        ProjectAst {
            root: String::new(),
            files: vec![FileAst {
                path: path.to_string(),
                imports: Vec::new(),
                assignments: Vec::new(),
                calls: Vec::<CallAst>::new(),
                symbols: Vec::new(),
                statements: vec![
                    statement(
                        AstStatementKind::Expression,
                        "start()",
                        owner_id,
                        span(0, 7),
                    ),
                    statement(
                        AstStatementKind::If,
                        if python {
                            "if flag:\n    return done\nelse:\n    alive()"
                        } else {
                            "if (flag) {\n  return done;\n} else {\n  alive();\n}"
                        },
                        owner_id,
                        span(10, 80),
                    ),
                    statement(
                        AstStatementKind::Return,
                        "return done",
                        owner_id,
                        span(25, 36),
                    ),
                    statement(
                        AstStatementKind::Expression,
                        "alive()",
                        owner_id,
                        span(58, 65),
                    ),
                    statement(
                        AstStatementKind::Expression,
                        "after_branch()",
                        owner_id,
                        span(84, 98),
                    ),
                    statement(
                        AstStatementKind::Loop,
                        if python {
                            "while keep:\n    if stop:\n        break\n        after_break()\n    continue\n    after_continue()"
                        } else {
                            "while (keep) {\n  if (stop) {\n    break;\n    after_break();\n  }\n  continue;\n  after_continue();\n}"
                        },
                        owner_id,
                        span(110, 220),
                    ),
                    statement(
                        AstStatementKind::If,
                        if python {
                            "if stop:\n        break\n        after_break()"
                        } else {
                            "if (stop) {\n    break;\n    after_break();\n  }"
                        },
                        owner_id,
                        span(130, 170),
                    ),
                    statement(AstStatementKind::Break, "break", owner_id, span(145, 150)),
                    statement(
                        AstStatementKind::Expression,
                        "after_break()",
                        owner_id,
                        span(152, 165),
                    ),
                    statement(
                        AstStatementKind::Continue,
                        "continue",
                        owner_id,
                        span(180, 188),
                    ),
                    statement(
                        AstStatementKind::Expression,
                        "after_continue()",
                        owner_id,
                        span(190, 206),
                    ),
                    statement(
                        AstStatementKind::Expression,
                        "after_loop()",
                        owner_id,
                        span(225, 237),
                    ),
                    statement(
                        AstStatementKind::Try,
                        if python {
                            "try:\n    risky()\n    raise Error()\n    after_raise()\nexcept Error:\n    handled()\nfinally:\n    cleanup()"
                        } else {
                            "try {\n  risky();\n  throw error;\n  after_raise();\n} catch (error) {\n  handled();\n} finally {\n  cleanup();\n}"
                        },
                        owner_id,
                        span(250, 390),
                    ),
                    statement(
                        AstStatementKind::Expression,
                        "risky()",
                        owner_id,
                        span(265, 272),
                    ),
                    statement(
                        if python {
                            AstStatementKind::Raise
                        } else {
                            AstStatementKind::Throw
                        },
                        if python {
                            "raise Error()"
                        } else {
                            "throw error"
                        },
                        owner_id,
                        span(280, 292),
                    ),
                    statement(
                        AstStatementKind::Expression,
                        "after_raise()",
                        owner_id,
                        span(294, 307),
                    ),
                    statement(AstStatementKind::Catch, "catch", owner_id, span(320, 345)),
                    statement(
                        AstStatementKind::Expression,
                        "handled()",
                        owner_id,
                        span(330, 339),
                    ),
                    statement(
                        AstStatementKind::Finally,
                        "finally",
                        owner_id,
                        span(350, 380),
                    ),
                    statement(
                        AstStatementKind::Expression,
                        "cleanup()",
                        owner_id,
                        span(360, 369),
                    ),
                    statement(
                        AstStatementKind::Expression,
                        "after_try()",
                        owner_id,
                        span(400, 411),
                    ),
                    statement(
                        AstStatementKind::Return,
                        "return final",
                        owner_id,
                        span(420, 432),
                    ),
                    statement(
                        AstStatementKind::Expression,
                        "after_return()",
                        owner_id,
                        span(440, 454),
                    ),
                ],
                expressions: vec![
                    expression(
                        AstExpressionKind::Identifier,
                        "flag",
                        owner_id,
                        span(13, 17),
                    ),
                    expression(
                        AstExpressionKind::Identifier,
                        "keep",
                        owner_id,
                        span(116, 120),
                    ),
                    expression(
                        AstExpressionKind::Identifier,
                        "stop",
                        owner_id,
                        span(133, 137),
                    ),
                ],
                conditions: vec![
                    ConditionAst {
                        kind: AstConditionKind::If,
                        text: "flag".to_string(),
                        owner_id: owner_id.to_string(),
                        source_span: span(13, 17),
                    },
                    ConditionAst {
                        kind: AstConditionKind::While,
                        text: "keep".to_string(),
                        owner_id: owner_id.to_string(),
                        source_span: span(116, 120),
                    },
                    ConditionAst {
                        kind: AstConditionKind::If,
                        text: "stop".to_string(),
                        owner_id: owner_id.to_string(),
                        source_span: span(133, 137),
                    },
                ],
                definitions: Vec::new(),
                uses: Vec::new(),
                returns: vec![
                    ReturnAst {
                        value: Some("done".to_string()),
                        owner_id: owner_id.to_string(),
                        source_span: span(25, 36),
                    },
                    ReturnAst {
                        value: Some("final".to_string()),
                        owner_id: owner_id.to_string(),
                        source_span: span(420, 432),
                    },
                ],
                raises: vec![RaiseAst {
                    text: if python {
                        "raise Error()".to_string()
                    } else {
                        "throw error".to_string()
                    },
                    value: Some(if python {
                        "Error()".to_string()
                    } else {
                        "error".to_string()
                    }),
                    owner_id: owner_id.to_string(),
                    source_span: span(280, 292),
                }],
                field_accesses: Vec::new(),
                index_accesses: Vec::new(),
            }],
        }
    }

    fn sg051_exit_project(path: &str, owner_id: &str, python: bool) -> ProjectAst {
        ProjectAst {
            root: String::new(),
            files: vec![FileAst {
                path: path.to_string(),
                imports: Vec::new(),
                assignments: Vec::new(),
                calls: Vec::<CallAst>::new(),
                symbols: Vec::new(),
                statements: vec![
                    statement(
                        AstStatementKind::Expression,
                        "start()",
                        owner_id,
                        span(0, 7),
                    ),
                    statement(
                        AstStatementKind::If,
                        if python {
                            "if fatal:\n    raise Fatal()"
                        } else {
                            "if (fatal) {\n  throw fatal;\n}"
                        },
                        owner_id,
                        span(10, 45),
                    ),
                    statement(
                        if python {
                            AstStatementKind::Raise
                        } else {
                            AstStatementKind::Throw
                        },
                        if python {
                            "raise Fatal()"
                        } else {
                            "throw fatal"
                        },
                        owner_id,
                        span(25, 38),
                    ),
                    statement(
                        AstStatementKind::Expression,
                        "after_if()",
                        owner_id,
                        span(50, 60),
                    ),
                    statement(
                        AstStatementKind::Try,
                        if python {
                            "try:\n    risky()\n    raise Error()\nexcept Error:\n    handled()\nfinally:\n    cleanup()"
                        } else {
                            "try {\n  risky();\n  throw error;\n} catch (error) {\n  handled();\n} finally {\n  cleanup();\n}"
                        },
                        owner_id,
                        span(70, 160),
                    ),
                    statement(
                        AstStatementKind::Expression,
                        "risky()",
                        owner_id,
                        span(82, 89),
                    ),
                    statement(
                        if python {
                            AstStatementKind::Raise
                        } else {
                            AstStatementKind::Throw
                        },
                        if python {
                            "raise Error()"
                        } else {
                            "throw error"
                        },
                        owner_id,
                        span(94, 106),
                    ),
                    statement(AstStatementKind::Catch, "catch", owner_id, span(110, 133)),
                    statement(
                        AstStatementKind::Expression,
                        "handled()",
                        owner_id,
                        span(118, 127),
                    ),
                    statement(
                        AstStatementKind::Finally,
                        "finally",
                        owner_id,
                        span(135, 157),
                    ),
                    statement(
                        AstStatementKind::Expression,
                        "cleanup()",
                        owner_id,
                        span(144, 153),
                    ),
                    statement(
                        AstStatementKind::Return,
                        "return done",
                        owner_id,
                        span(170, 181),
                    ),
                ],
                expressions: vec![expression(
                    AstExpressionKind::Identifier,
                    "fatal",
                    owner_id,
                    span(13, 18),
                )],
                conditions: vec![ConditionAst {
                    kind: AstConditionKind::If,
                    text: "fatal".to_string(),
                    owner_id: owner_id.to_string(),
                    source_span: span(13, 18),
                }],
                definitions: Vec::new(),
                uses: Vec::new(),
                returns: vec![ReturnAst {
                    value: Some("done".to_string()),
                    owner_id: owner_id.to_string(),
                    source_span: span(170, 181),
                }],
                raises: vec![
                    RaiseAst {
                        text: if python {
                            "raise Fatal()".to_string()
                        } else {
                            "throw fatal".to_string()
                        },
                        value: Some(if python {
                            "Fatal()".to_string()
                        } else {
                            "fatal".to_string()
                        }),
                        owner_id: owner_id.to_string(),
                        source_span: span(25, 38),
                    },
                    RaiseAst {
                        text: if python {
                            "raise Error()".to_string()
                        } else {
                            "throw error".to_string()
                        },
                        value: Some(if python {
                            "Error()".to_string()
                        } else {
                            "error".to_string()
                        }),
                        owner_id: owner_id.to_string(),
                        source_span: span(94, 106),
                    },
                ],
                field_accesses: Vec::new(),
                index_accesses: Vec::new(),
            }],
        }
    }

    fn sg051_finally_project(path: &str, owner_id: &str, python: bool) -> ProjectAst {
        ProjectAst {
            root: String::new(),
            files: vec![FileAst {
                path: path.to_string(),
                imports: Vec::new(),
                assignments: Vec::new(),
                calls: Vec::<CallAst>::new(),
                symbols: Vec::new(),
                statements: vec![
                    statement(
                        AstStatementKind::Try,
                        if python {
                            "try:\n    raise Fatal()\nfinally:\n    cleanup()"
                        } else {
                            "try {\n  throw fatal;\n} finally {\n  cleanup();\n}"
                        },
                        owner_id,
                        span(0, 80),
                    ),
                    statement(
                        if python {
                            AstStatementKind::Raise
                        } else {
                            AstStatementKind::Throw
                        },
                        if python {
                            "raise Fatal()"
                        } else {
                            "throw fatal"
                        },
                        owner_id,
                        span(12, 24),
                    ),
                    statement(AstStatementKind::Finally, "finally", owner_id, span(40, 70)),
                    statement(
                        AstStatementKind::Expression,
                        "cleanup()",
                        owner_id,
                        span(50, 59),
                    ),
                ],
                expressions: Vec::new(),
                conditions: Vec::new(),
                definitions: Vec::new(),
                uses: Vec::new(),
                returns: Vec::new(),
                raises: vec![RaiseAst {
                    text: if python {
                        "raise Fatal()".to_string()
                    } else {
                        "throw fatal".to_string()
                    },
                    value: Some(if python {
                        "Fatal()".to_string()
                    } else {
                        "fatal".to_string()
                    }),
                    owner_id: owner_id.to_string(),
                    source_span: span(12, 24),
                }],
                field_accesses: Vec::new(),
                index_accesses: Vec::new(),
            }],
        }
    }

    fn sg121_switch_match_fallback_project(path: &str, owner_id: &str, python: bool) -> ProjectAst {
        ProjectAst {
            root: String::new(),
            files: vec![FileAst {
                path: path.to_string(),
                imports: Vec::new(),
                assignments: Vec::new(),
                calls: Vec::<CallAst>::new(),
                symbols: Vec::new(),
                statements: vec![statement(
                    AstStatementKind::Unknown,
                    if python {
                        "match status:\n    case 'ready':\n        ready()"
                    } else {
                        "switch (status) {\n  case 'ready':\n    ready();\n}"
                    },
                    owner_id,
                    span(0, 48),
                )],
                expressions: Vec::new(),
                conditions: Vec::new(),
                definitions: Vec::new(),
                uses: Vec::new(),
                returns: Vec::new(),
                raises: Vec::new(),
                field_accesses: Vec::new(),
                index_accesses: Vec::new(),
            }],
        }
    }

    fn statements(owner_id: &str, python: bool) -> Vec<StatementAst> {
        vec![
            statement(
                AstStatementKind::Expression,
                "start()",
                owner_id,
                span(0, 7),
            ),
            statement(
                AstStatementKind::If,
                if python {
                    "if flag:\n    success()\nelse:\n    recover()"
                } else {
                    "if (flag) {\n  success();\n} else {\n  recover();\n}"
                },
                owner_id,
                span(10, 80),
            ),
            statement(
                AstStatementKind::Expression,
                "success()",
                owner_id,
                span(25, 34),
            ),
            statement(
                AstStatementKind::Expression,
                "recover()",
                owner_id,
                span(58, 67),
            ),
            statement(
                AstStatementKind::Expression,
                "after_if()",
                owner_id,
                span(82, 92),
            ),
            statement(
                AstStatementKind::Loop,
                if python {
                    "while keep:\n    tick()\n    if stop:\n        break\n    continue"
                } else {
                    "while (keep) {\n  tick();\n  if (stop) {\n    break;\n  }\n  continue;\n}"
                },
                owner_id,
                span(94, 180),
            ),
            statement(
                AstStatementKind::Expression,
                "tick()",
                owner_id,
                span(112, 118),
            ),
            statement(
                AstStatementKind::If,
                if python {
                    "if stop:\n    break"
                } else {
                    "if (stop) {\n    break;\n  }"
                },
                owner_id,
                span(122, 152),
            ),
            statement(AstStatementKind::Break, "break", owner_id, span(140, 145)),
            statement(
                AstStatementKind::Continue,
                "continue",
                owner_id,
                span(160, 168),
            ),
            statement(
                AstStatementKind::Expression,
                "after_loop()",
                owner_id,
                span(182, 194),
            ),
            statement(
                AstStatementKind::Try,
                if python {
                    "try:\n    risky()\n    raise Error()\nexcept Error:\n    handled()\nfinally:\n    cleanup()"
                } else {
                    "try {\n  risky();\n  throw error;\n} catch (error) {\n  handled();\n} finally {\n  cleanup();\n}"
                },
                owner_id,
                span(200, 290),
            ),
            statement(
                AstStatementKind::Expression,
                "risky()",
                owner_id,
                span(212, 219),
            ),
            statement(
                if python {
                    AstStatementKind::Raise
                } else {
                    AstStatementKind::Throw
                },
                if python {
                    "raise Error()"
                } else {
                    "throw error"
                },
                owner_id,
                span(224, 236),
            ),
            statement(AstStatementKind::Catch, "catch", owner_id, span(240, 263)),
            statement(
                AstStatementKind::Expression,
                "handled()",
                owner_id,
                span(248, 257),
            ),
            statement(
                AstStatementKind::Finally,
                "finally",
                owner_id,
                span(265, 287),
            ),
            statement(
                AstStatementKind::Expression,
                "cleanup()",
                owner_id,
                span(274, 283),
            ),
            statement(
                AstStatementKind::Return,
                "return done",
                owner_id,
                span(300, 311),
            ),
            statement(
                AstStatementKind::Expression,
                "unreachable_after_return()",
                owner_id,
                span(320, 346),
            ),
            statement(
                AstStatementKind::Expression,
                if python {
                    "ready and enabled"
                } else {
                    "ready && enabled"
                },
                owner_id,
                span(350, 370),
            ),
        ]
    }

    fn expressions(owner_id: &str, python: bool) -> Vec<ExpressionAst> {
        vec![
            expression(
                AstExpressionKind::Identifier,
                "flag",
                owner_id,
                span(15, 19),
            ),
            expression(
                AstExpressionKind::Identifier,
                "keep",
                owner_id,
                span(100, 104),
            ),
            expression(
                AstExpressionKind::Identifier,
                "stop",
                owner_id,
                span(126, 130),
            ),
            expression(
                AstExpressionKind::BinaryOperator,
                if python {
                    "ready and enabled"
                } else {
                    "ready && enabled"
                },
                owner_id,
                span(350, 370),
            ),
        ]
    }

    fn conditions(owner_id: &str) -> Vec<ConditionAst> {
        vec![
            ConditionAst {
                kind: AstConditionKind::If,
                text: "flag".to_string(),
                owner_id: owner_id.to_string(),
                source_span: span(15, 19),
            },
            ConditionAst {
                kind: AstConditionKind::While,
                text: "keep".to_string(),
                owner_id: owner_id.to_string(),
                source_span: span(100, 104),
            },
            ConditionAst {
                kind: AstConditionKind::If,
                text: "stop".to_string(),
                owner_id: owner_id.to_string(),
                source_span: span(126, 130),
            },
        ]
    }

    fn statement(
        kind: AstStatementKind,
        text: &str,
        owner_id: &str,
        source_span: SourceSpan,
    ) -> StatementAst {
        StatementAst {
            kind,
            text: text.to_string(),
            owner_id: owner_id.to_string(),
            source_span,
        }
    }

    fn expression(
        kind: AstExpressionKind,
        text: &str,
        owner_id: &str,
        source_span: SourceSpan,
    ) -> ExpressionAst {
        ExpressionAst {
            kind,
            text: text.to_string(),
            owner_id: owner_id.to_string(),
            source_span,
        }
    }

    fn span(start_byte: usize, end_byte: usize) -> SourceSpan {
        SourceSpan {
            start_byte,
            end_byte,
            start_row: start_byte,
            start_column: 0,
            end_row: end_byte,
            end_column: 0,
        }
    }
}
