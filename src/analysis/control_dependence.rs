use crate::intern::Sym;
use std::collections::{BTreeMap, BTreeSet};

use crate::ast::SourceSpan;
use crate::supergraph::{
    self as sg, Confidence, ControlFlowNodeRole, DataFlowNodeRole, EdgeFact, EdgeKind, GraphNode,
    NodeFact, NodeId, ProgramSupergraph,
};

use super::{
    SemanticCallable, SemanticContext, callable_index::CallableIndex, edge_id, graph_edge,
    inference_evidence, insert_edge, node_owner, span_contains,
};

const PRECISION: &str = "sg061-post-dominance-control-dependence";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallableCfg {
    pub callable_id: NodeId,
    pub node_ids: BTreeSet<NodeId>,
    pub reachable_node_ids: BTreeSet<NodeId>,
    pub entry_node_id: NodeId,
    pub normal_exit_node_id: Option<NodeId>,
    pub exceptional_exit_node_id: Option<NodeId>,
    pub exit_node_ids: BTreeSet<NodeId>,
    pub successors: BTreeMap<NodeId, BTreeSet<NodeId>>,
    pub predecessors: BTreeMap<NodeId, BTreeSet<NodeId>>,
}

impl CallableCfg {
    pub fn successors_of(&self, node_id: NodeId) -> BTreeSet<NodeId> {
        self.successors.get(&node_id).cloned().unwrap_or_default()
    }

    pub fn predecessors_of(&self, node_id: NodeId) -> BTreeSet<NodeId> {
        self.predecessors.get(&node_id).cloned().unwrap_or_default()
    }

    pub fn is_reachable(&self, node_id: NodeId) -> bool {
        self.reachable_node_ids.contains(&node_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DominatorSets {
    pub root_ids: BTreeSet<NodeId>,
    pub node_ids: BTreeSet<NodeId>,
    pub dominators: BTreeMap<NodeId, BTreeSet<NodeId>>,
    pub immediate_dominators: BTreeMap<NodeId, Option<NodeId>>,
}

impl DominatorSets {
    pub fn dominators_of(&self, node_id: NodeId) -> Option<&BTreeSet<NodeId>> {
        self.dominators.get(&node_id)
    }

    pub fn immediate_dominator_of(&self, node_id: NodeId) -> Option<&NodeId> {
        self.immediate_dominators
            .get(&node_id)
            .and_then(Option::as_ref)
    }

    pub fn dominates(&self, dominator_id: NodeId, node_id: NodeId) -> bool {
        self.dominators
            .get(&node_id)
            .is_some_and(|dominators| dominators.contains(&dominator_id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallableDominance {
    pub cfg: CallableCfg,
    pub dominators: DominatorSets,
    pub post_dominators: DominatorSets,
}

impl CallableDominance {
    pub fn dominates(&self, dominator_id: NodeId, node_id: NodeId) -> bool {
        self.dominators.dominates(dominator_id, node_id)
    }

    pub fn post_dominates(&self, post_dominator_id: NodeId, node_id: NodeId) -> bool {
        self.post_dominators.dominates(post_dominator_id, node_id)
    }
}

pub fn analyze_callable_dominance(
    graph: &ProgramSupergraph,
    callable_id: NodeId,
) -> Option<CallableDominance> {
    dominance_for_cfg(callable_cfg(graph, callable_id)?)
}

pub(crate) fn analyze_callable_dominance_indexed(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: NodeId,
) -> Option<CallableDominance> {
    dominance_for_cfg(callable_cfg_indexed(graph, index, callable_id)?)
}

fn dominance_for_cfg(cfg: CallableCfg) -> Option<CallableDominance> {
    let dominators = compute_dominators(
        cfg.reachable_node_ids.clone(),
        BTreeSet::from([cfg.entry_node_id]),
        &cfg.predecessors,
    );

    let post_dominator_roots = cfg
        .exit_node_ids
        .intersection(&cfg.reachable_node_ids)
        .cloned()
        .collect::<BTreeSet<_>>();
    let terminal_roots = if post_dominator_roots.is_empty() {
        cfg.reachable_node_ids
            .iter()
            .filter(|node_id| {
                cfg.successors
                    .get(*node_id)
                    .is_none_or(|successors| successors.is_empty())
            })
            .cloned()
            .collect::<BTreeSet<_>>()
    } else {
        post_dominator_roots
    };
    let post_dominator_nodes = reverse_reachable(&terminal_roots, &cfg.predecessors)
        .intersection(&cfg.reachable_node_ids)
        .cloned()
        .collect::<BTreeSet<_>>();
    let post_dominators = compute_dominators(post_dominator_nodes, terminal_roots, &cfg.successors);

    Some(CallableDominance {
        cfg,
        dominators,
        post_dominators,
    })
}

pub fn callable_cfg(graph: &ProgramSupergraph, callable_id: NodeId) -> Option<CallableCfg> {
    callable_cfg_from(
        callable_id,
        indexed_control_flow_nodes(graph).into_iter(),
        indexed_control_flow_edges(graph).into_iter(),
    )
}

pub(crate) fn callable_cfg_indexed(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: NodeId,
) -> Option<CallableCfg> {
    callable_cfg_from(
        callable_id,
        index.nodes(graph, callable_id),
        index.control_flow_edges(graph, callable_id),
    )
}

fn callable_cfg_from<'g>(
    callable_id: NodeId,
    nodes: impl Iterator<Item = &'g GraphNode>,
    edges: impl Iterator<Item = &'g sg::GraphEdge>,
) -> Option<CallableCfg> {
    let mut node_ids = BTreeSet::new();
    let mut entry_node_id = None;
    let mut normal_exit_node_id = None;
    let mut exceptional_exit_node_id = None;
    let mut exit_node_ids = BTreeSet::new();

    for node in nodes {
        let NodeFact::ControlFlow(control) = &node.fact else {
            continue;
        };
        if control.callable_id != callable_id {
            continue;
        }
        node_ids.insert(control.cfg_node_id);
        match control.role {
            ControlFlowNodeRole::Entry => entry_node_id = Some(control.cfg_node_id),
            ControlFlowNodeRole::Exit => {
                exit_node_ids.insert(control.cfg_node_id);
                match control.semantic_kind.as_deref() {
                    Some("NormalExit") => normal_exit_node_id = Some(control.cfg_node_id),
                    Some("ExceptionalExit") => {
                        exceptional_exit_node_id = Some(control.cfg_node_id)
                    }
                    _ => {}
                }
            }
            ControlFlowNodeRole::Statement
            | ControlFlowNodeRole::Condition
            | ControlFlowNodeRole::Merge
            | ControlFlowNodeRole::Return
            | ControlFlowNodeRole::Raise => {}
        }
    }

    let entry_node_id = entry_node_id?;
    let mut successors = node_ids
        .iter()
        .map(|node_id| (*node_id, BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    let mut predecessors = successors.clone();

    for edge in edges {
        let EdgeFact::ControlFlow(flow) = &edge.fact else {
            continue;
        };
        if flow.callable_id != callable_id {
            continue;
        }
        let Some(target_id) = edge.target_id else {
            continue;
        };
        if !node_ids.contains(&edge.source_id) || !node_ids.contains(&target_id) {
            continue;
        }
        successors
            .entry(edge.source_id)
            .or_default()
            .insert(target_id);
        predecessors
            .entry(target_id)
            .or_default()
            .insert(edge.source_id);
    }

    let reachable_node_ids = reachable_from(&BTreeSet::from([entry_node_id]), &successors);

    Some(CallableCfg {
        callable_id,
        node_ids,
        reachable_node_ids,
        entry_node_id,
        normal_exit_node_id,
        exceptional_exit_node_id,
        exit_node_ids,
        successors,
        predecessors,
    })
}

fn compute_dominators(
    node_ids: BTreeSet<NodeId>,
    root_ids: BTreeSet<NodeId>,
    incoming: &BTreeMap<NodeId, BTreeSet<NodeId>>,
) -> DominatorSets {
    let root_ids = root_ids
        .intersection(&node_ids)
        .cloned()
        .collect::<BTreeSet<_>>();

    // Dense indices follow the sorted order of `node_ids`, so ascending bit order matches
    // the iteration order of the `BTreeSet`s that are materialized at the end.
    let ordered = node_ids.iter().copied().collect::<Vec<_>>();
    let position = ordered
        .iter()
        .enumerate()
        .map(|(index, node_id)| (node_id, index))
        .collect::<BTreeMap<_, _>>();
    let words = ordered.len().div_ceil(64);
    let is_root = ordered
        .iter()
        .map(|node_id| root_ids.contains(node_id))
        .collect::<Vec<_>>();
    let predecessors = ordered
        .iter()
        .map(|node_id| {
            incoming
                .get(node_id)
                .into_iter()
                .flatten()
                .filter_map(|predecessor| position.get(predecessor).copied())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    let mut sets = (0..ordered.len())
        .map(|index| {
            if is_root[index] {
                singleton(words, index)
            } else {
                full(words, ordered.len())
            }
        })
        .collect::<Vec<_>>();

    let mut changed = true;
    while changed {
        changed = false;
        for index in 0..ordered.len() {
            if is_root[index] {
                continue;
            }
            let mut next = match predecessors[index].split_first() {
                Some((first, rest)) => {
                    let mut intersection = sets[*first].clone();
                    for predecessor in rest {
                        for (word, other) in intersection.iter_mut().zip(&sets[*predecessor]) {
                            *word &= *other;
                        }
                    }
                    intersection
                }
                None => vec![0; words],
            };
            next[index / 64] |= 1 << (index % 64);
            if sets[index] != next {
                sets[index] = next;
                changed = true;
            }
        }
    }

    let immediate_dominators = ordered
        .iter()
        .enumerate()
        .map(|(index, node_id)| {
            if is_root[index] {
                return ((*node_id), None);
            }
            let mut strict = sets[index].clone();
            strict[index / 64] &= !(1 << (index % 64));
            let idom = bits(&strict).find(|candidate| {
                let mut others = strict.clone();
                others[candidate / 64] &= !(1 << (candidate % 64));
                others
                    .iter()
                    .zip(&sets[*candidate])
                    .all(|(other, dominators)| other & !dominators == 0)
            });
            ((*node_id), idom.map(|candidate| ordered[candidate].clone()))
        })
        .collect::<BTreeMap<_, _>>();

    let dominators = ordered
        .iter()
        .enumerate()
        .map(|(index, node_id)| {
            (
                (*node_id),
                bits(&sets[index])
                    .map(|member| ordered[member].clone())
                    .collect::<BTreeSet<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    DominatorSets {
        root_ids,
        node_ids,
        dominators,
        immediate_dominators,
    }
}

fn singleton(words: usize, index: usize) -> Vec<u64> {
    let mut set = vec![0; words];
    set[index / 64] |= 1 << (index % 64);
    set
}

fn full(words: usize, len: usize) -> Vec<u64> {
    let mut set = vec![u64::MAX; words];
    if len % 64 != 0 {
        set[words - 1] = (1 << (len % 64)) - 1;
    }
    set
}

fn bits(set: &[u64]) -> impl Iterator<Item = usize> + '_ {
    set.iter().enumerate().flat_map(|(word_index, word)| {
        (0..64)
            .filter(move |bit| word & (1 << bit) != 0)
            .map(move |bit| word_index * 64 + bit)
    })
}

fn reachable_from(
    roots: &BTreeSet<NodeId>,
    successors: &BTreeMap<NodeId, BTreeSet<NodeId>>,
) -> BTreeSet<NodeId> {
    let mut reachable = BTreeSet::new();
    let mut pending = roots.iter().cloned().collect::<Vec<_>>();
    while let Some(node_id) = pending.pop() {
        if !reachable.insert(node_id) {
            continue;
        }
        if let Some(next) = successors.get(&node_id) {
            pending.extend(next.iter().rev().cloned());
        }
    }
    reachable
}

fn reverse_reachable(
    roots: &BTreeSet<NodeId>,
    predecessors: &BTreeMap<NodeId, BTreeSet<NodeId>>,
) -> BTreeSet<NodeId> {
    reachable_from(roots, predecessors)
}

fn indexed_control_flow_nodes(graph: &ProgramSupergraph) -> Vec<&sg::GraphNode> {
    if let Some(node_ids) = graph.indexes.nodes_by_kind.get(&sg::NodeKind::ControlFlow) {
        return node_ids
            .iter()
            .filter_map(|node_id| {
                graph
                    .indexes
                    .node_position_by_id
                    .get(node_id)
                    .and_then(|position| graph.nodes.get(*position as usize))
            })
            .collect();
    }
    graph
        .nodes
        .iter()
        .filter(|node| node.kind == sg::NodeKind::ControlFlow)
        .collect()
}

fn indexed_control_flow_edges(graph: &ProgramSupergraph) -> Vec<&sg::GraphEdge> {
    if let Some(edge_ids) = graph.indexes.edges_by_kind.get(&EdgeKind::ControlFlow) {
        return edge_ids
            .iter()
            .filter_map(|edge_id| {
                graph
                    .indexes
                    .edge_position_by_id
                    .get(edge_id)
                    .and_then(|position| graph.edges.get(*position as usize))
            })
            .collect();
    }
    graph
        .edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::ControlFlow)
        .collect()
}

pub(crate) fn emit(graph: &mut ProgramSupergraph, context: &SemanticContext<'_>) {
    graph
        .edges
        .retain(|edge| !matches!(edge.fact, EdgeFact::Controls(_)));
    graph.invalidate_id_cache();
    let index = CallableIndex::build(graph);

    for semantic in context.semantic_callables() {
        emit_callable(graph, &index, context, &semantic);
    }
}

fn emit_callable(
    graph: &mut ProgramSupergraph,
    index: &CallableIndex,
    _context: &SemanticContext<'_>,
    semantic: &SemanticCallable<'_>,
) {
    let Some(analysis) = analyze_callable_dominance_indexed(graph, index, semantic.callable().callable_id)
    else {
        return;
    };

    let mut controls = BTreeSet::<(NodeId, NodeId, Option<SourceSpan>)>::new();
    for condition_id in branch_condition_ids(graph, index, &analysis) {
        for cfg_node_id in controlled_cfg_node_ids(&analysis, condition_id) {
            for target in controlled_targets_for_cfg_node(graph, index, semantic, cfg_node_id) {
                controls.insert((condition_id, target.target_id, target.span));
            }
        }
    }

    for (condition_id, controlled_id, span) in controls {
        add_controls_edge(graph, semantic, condition_id, controlled_id, span);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ControlledTarget {
    target_id: NodeId,
    span: Option<SourceSpan>,
}

fn branch_condition_ids(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    analysis: &CallableDominance,
) -> BTreeSet<NodeId> {
    index
        .nodes(graph, analysis.cfg.callable_id)
        .filter_map(|node| match &node.fact {
            NodeFact::ControlFlow(control)
                if control.role == ControlFlowNodeRole::Condition
                    && analysis.cfg.is_reachable(control.cfg_node_id)
                    && analysis.cfg.successors_of(control.cfg_node_id).len() >= 2 =>
            {
                Some(control.cfg_node_id)
            }
            _ => None,
        })
        .collect()
}

fn controlled_cfg_node_ids(analysis: &CallableDominance, condition_id: NodeId) -> BTreeSet<NodeId> {
    let mut controlled = BTreeSet::new();
    for successor_id in analysis.cfg.successors_of(condition_id) {
        if !analysis.cfg.is_reachable(successor_id) {
            continue;
        }
        if analysis.post_dominates(successor_id, condition_id) {
            continue;
        }
        for candidate_id in analysis
            .post_dominators
            .dominators_of(successor_id)
            .into_iter()
            .flatten()
        {
            if *candidate_id == condition_id {
                continue;
            }
            if analysis.post_dominates(*candidate_id, condition_id) {
                continue;
            }
            controlled.insert(*candidate_id);
        }
    }
    controlled
}

fn controlled_targets_for_cfg_node(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    semantic: &SemanticCallable<'_>,
    cfg_node_id: NodeId,
) -> Vec<ControlledTarget> {
    let Some((cfg_node, control)) = cfg_control_node(graph, index, cfg_node_id) else {
        return Vec::new();
    };
    match control.role {
        ControlFlowNodeRole::Condition
        | ControlFlowNodeRole::Return
        | ControlFlowNodeRole::Raise => {
            vec![ControlledTarget {
                target_id: control.cfg_node_id,
                span: cfg_node.span,
            }]
        }
        ControlFlowNodeRole::Statement => {
            let Some(statement_span) = cfg_node.span else {
                return Vec::new();
            };
            let mut targets = Vec::new();
            targets.extend(statement_targets_for_cfg_statement(
                graph,
                index,
                semantic.callable().callable_id,
                cfg_node_id,
                statement_span,
            ));
            targets.extend(direct_fact_targets_for_cfg_statement(
                graph,
                index,
                semantic.callable().callable_id,
                cfg_node_id,
                statement_span,
            ));
            targets
        }
        ControlFlowNodeRole::Entry | ControlFlowNodeRole::Exit | ControlFlowNodeRole::Merge => {
            Vec::new()
        }
    }
}

fn statement_targets_for_cfg_statement(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: NodeId,
    cfg_node_id: NodeId,
    statement_span: SourceSpan,
) -> Vec<ControlledTarget> {
    index
        .nodes(graph, callable_id)
        .filter_map(|node| match &node.fact {
            NodeFact::Statement(statement)
                if node.span == Some(statement_span)
                    && nearest_statement_cfg_node_for_span(graph, index, callable_id, statement_span)
                        
                        == Some(cfg_node_id) =>
            {
                Some(ControlledTarget {
                    target_id: statement.statement_id,
                    span: node.span,
                })
            }
            _ => None,
        })
        .collect()
}

fn direct_fact_targets_for_cfg_statement(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: NodeId,
    cfg_node_id: NodeId,
    statement_span: SourceSpan,
) -> Vec<ControlledTarget> {
    index
        .nodes(graph, callable_id)
        .filter_map(|node| {
            let span = node.span?;
            if !span_contains(statement_span, span)
                || nearest_statement_cfg_node_for_span(graph, index, callable_id, span)
                    != Some(cfg_node_id)
            {
                return None;
            }
            match &node.fact {
                NodeFact::CallSite(call_site) if call_site.enclosing_callable_id == callable_id => {
                    Some(ControlledTarget {
                        target_id: call_site.call_site_id,
                        span: node.span,
                    })
                }
                NodeFact::Definition(definition) if definition.callable_id == callable_id => {
                    Some(ControlledTarget {
                        target_id: definition.definition_id,
                        span: node.span,
                    })
                }
                NodeFact::DataFlow(data_flow)
                    if data_flow.callable_id == callable_id
                        && data_flow.role == DataFlowNodeRole::Definition =>
                {
                    Some(ControlledTarget {
                        target_id: data_flow.data_flow_node_id,
                        span: node.span,
                    })
                }
                NodeFact::Expression(expression) if expression.callable_id == callable_id => {
                    Some(ControlledTarget {
                        target_id: expression.expression_id,
                        span: node.span,
                    })
                }
                _ => None,
            }
        })
        .collect()
}

fn nearest_statement_cfg_node_for_span(
    graph: &ProgramSupergraph,
    index: &CallableIndex,
    callable_id: NodeId,
    span: SourceSpan,
) -> Option<NodeId> {
    index
        .nodes(graph, callable_id)
        .filter_map(|node| match &node.fact {
            NodeFact::ControlFlow(control)
                if control.role == ControlFlowNodeRole::Statement
                    && node
                        .span
                        .is_some_and(|statement_span| span_contains(statement_span, span)) =>
            {
                let statement_span = node.span?;
                Some((
                    control.cfg_node_id,
                    statement_span
                        .end_byte
                        .saturating_sub(statement_span.start_byte),
                    statement_span.start_byte,
                    statement_span.end_byte,
                ))
            }
            _ => None,
        })
        .min_by_key(|(_, length, start, end)| (*length, *start, *end))
        .map(|(node_id, _, _, _)| node_id)
}

fn cfg_control_node<'a>(
    graph: &'a ProgramSupergraph,
    index: &CallableIndex,
    cfg_node_id: NodeId,
) -> Option<(&'a GraphNode, &'a sg::ControlFlowNode)> {
    index
        .control_flow_node(graph, cfg_node_id)
        .and_then(|node| match &node.fact {
            NodeFact::ControlFlow(control) => Some((node, control)),
            _ => None,
        })
}

fn add_controls_edge(
    graph: &mut ProgramSupergraph,
    semantic: &SemanticCallable<'_>,
    condition_id: NodeId,
    controlled_id: NodeId,
    span: Option<SourceSpan>,
) {
    insert_edge(
        graph,
        graph_edge(
            edge_id("controls", condition_id, controlled_id, PRECISION),
            EdgeKind::Controls,
            condition_id,
            controlled_id,
            node_owner(semantic),
            span,
            Confidence::Exact,
            inference_evidence(
                "control dependence derived from CFG branch successor post-dominance",
            ),
            EdgeFact::Controls(sg::Controls {
                callable_id: semantic.callable().callable_id,
                condition_id,
                controlled_id,
                precision: Sym::from(PRECISION.to_string()),
            }),
        ),
    );
}

#[cfg(test)]
mod tests {
    use crate::intern::Sym;
    use std::collections::BTreeSet;

    use crate::analysis::source_graph::{build_python_supergraph, build_typescript_supergraph};
    use crate::ast::{
        CallAst, CallContext, ConditionAst, ConditionKind as AstConditionKind, DefinitionAst,
        DefinitionKind, ExpressionAst, ExpressionKind as AstExpressionKind, FileAst, ProjectAst,
        RaiseAst, ReturnAst, SourceSpan, StatementAst, StatementKind as AstStatementKind,
    };
    use crate::supergraph::{
        self as sg, Confidence, ControlFlowKind, ControlFlowNodeRole, ControlFlowOutcome, EdgeFact,
        EdgeKind, EvidenceKind, GraphEdge, GraphNode, NodeFact, NodeId, NodeKind,
        ProgramDependenceGraphFilter, ProgramSupergraph, SCHEMA_VERSION, SourceOwnership,
        Uncertainty, build_indexes,
    };

    use super::{PRECISION, analyze_callable_dominance};
    use crate::supergraph::ids::{callable_id_from_text, test_support::{test_edge_id, test_id}};

    const CALLABLE_ID: &str = "sample:<module>";

    fn callable_id() -> NodeId {
        callable_id_from_text(CALLABLE_ID)
    }

    #[test]
    fn sg060_computes_dominators_for_branches_loops_and_merges() {
        let graph = graph_fixture(
            &[
                ("entry", ControlFlowNodeRole::Entry, None),
                ("if", ControlFlowNodeRole::Condition, None),
                ("then", ControlFlowNodeRole::Statement, None),
                ("else", ControlFlowNodeRole::Statement, None),
                ("merge", ControlFlowNodeRole::Merge, None),
                ("loop", ControlFlowNodeRole::Condition, None),
                ("body", ControlFlowNodeRole::Statement, None),
                ("normal-exit", ControlFlowNodeRole::Exit, Some("NormalExit")),
                (
                    "exceptional-exit",
                    ControlFlowNodeRole::Exit,
                    Some("ExceptionalExit"),
                ),
            ],
            &[
                ("entry", "if", ControlFlowKind::Entry),
                ("if", "then", ControlFlowKind::Branch),
                ("if", "else", ControlFlowKind::Branch),
                ("then", "merge", ControlFlowKind::Sequential),
                ("else", "merge", ControlFlowKind::Sequential),
                ("merge", "loop", ControlFlowKind::Sequential),
                ("loop", "body", ControlFlowKind::Branch),
                ("body", "loop", ControlFlowKind::LoopBack),
                ("loop", "normal-exit", ControlFlowKind::Branch),
            ],
        );

        let analysis = analyze_callable_dominance(&graph, callable_id()).expect("dominance");

        assert!(analysis.dominates(test_id("entry"), test_id("body")));
        assert!(analysis.dominates(test_id("if"), test_id("merge")));
        assert!(analysis.dominates(test_id("merge"), test_id("loop")));
        assert!(analysis.dominates(test_id("loop"), test_id("body")));
        assert!(!analysis.dominates(test_id("then"), test_id("merge")));
        assert_eq!(
            analysis.dominators.immediate_dominator_of(test_id("merge")),
            Some(&test_id("if"))
        );
        assert_eq!(
            analysis.dominators.immediate_dominator_of(test_id("body")),
            Some(&test_id("loop"))
        );

        assert!(analysis.post_dominates(test_id("merge"), test_id("then")));
        assert!(analysis.post_dominates(test_id("merge"), test_id("else")));
        assert!(analysis.post_dominates(test_id("loop"), test_id("merge")));
        assert!(analysis.post_dominates(test_id("loop"), test_id("body")));
        assert!(analysis.post_dominates(test_id("normal-exit"), test_id("loop")));
        assert_eq!(
            analysis.post_dominators.immediate_dominator_of(test_id("then")),
            Some(&test_id("merge"))
        );
    }

    #[test]
    fn sg060_computes_post_dominance_for_normal_and_exceptional_terminals() {
        let graph = graph_fixture(
            &[
                ("entry", ControlFlowNodeRole::Entry, None),
                ("condition", ControlFlowNodeRole::Condition, None),
                ("return", ControlFlowNodeRole::Return, None),
                ("raise", ControlFlowNodeRole::Raise, None),
                ("normal-exit", ControlFlowNodeRole::Exit, Some("NormalExit")),
                (
                    "exceptional-exit",
                    ControlFlowNodeRole::Exit,
                    Some("ExceptionalExit"),
                ),
            ],
            &[
                ("entry", "condition", ControlFlowKind::Entry),
                ("condition", "return", ControlFlowKind::Branch),
                ("condition", "raise", ControlFlowKind::Branch),
                ("return", "normal-exit", ControlFlowKind::Exit),
                ("raise", "exceptional-exit", ControlFlowKind::Exit),
            ],
        );

        let analysis = analyze_callable_dominance(&graph, callable_id()).expect("dominance");

        assert!(analysis.post_dominates(test_id("normal-exit"), test_id("return")));
        assert!(analysis.post_dominates(test_id("exceptional-exit"), test_id("raise")));
        assert!(!analysis.post_dominates(test_id("normal-exit"), test_id("condition")));
        assert!(!analysis.post_dominates(test_id("exceptional-exit"), test_id("condition")));
        assert_eq!(
            analysis.post_dominators.dominators_of(test_id("condition")),
            Some(&BTreeSet::from([test_id("condition")]))
        );
    }

    #[test]
    fn sg060_tracks_handled_exception_paths_to_normal_exit() {
        let graph = graph_fixture(
            &[
                ("entry", ControlFlowNodeRole::Entry, None),
                ("try", ControlFlowNodeRole::Condition, None),
                ("risky", ControlFlowNodeRole::Statement, None),
                ("raise", ControlFlowNodeRole::Raise, None),
                ("catch", ControlFlowNodeRole::Statement, None),
                ("handled", ControlFlowNodeRole::Statement, None),
                ("normal-exit", ControlFlowNodeRole::Exit, Some("NormalExit")),
                (
                    "exceptional-exit",
                    ControlFlowNodeRole::Exit,
                    Some("ExceptionalExit"),
                ),
            ],
            &[
                ("entry", "try", ControlFlowKind::Entry),
                ("try", "risky", ControlFlowKind::Branch),
                ("risky", "raise", ControlFlowKind::Sequential),
                ("raise", "catch", ControlFlowKind::Branch),
                ("catch", "handled", ControlFlowKind::Sequential),
                ("handled", "normal-exit", ControlFlowKind::Exit),
            ],
        );

        let analysis = analyze_callable_dominance(&graph, callable_id()).expect("dominance");

        assert!(analysis.post_dominates(test_id("catch"), test_id("raise")));
        assert!(analysis.post_dominates(test_id("handled"), test_id("catch")));
        assert!(analysis.post_dominates(test_id("normal-exit"), test_id("raise")));
        assert!(!analysis.post_dominates(test_id("exceptional-exit"), test_id("raise")));
    }

    #[test]
    fn sg060_excludes_unreachable_cfg_nodes_from_dominance_sets() {
        let graph = graph_fixture(
            &[
                ("entry", ControlFlowNodeRole::Entry, None),
                ("live", ControlFlowNodeRole::Statement, None),
                ("dead", ControlFlowNodeRole::Statement, None),
                ("normal-exit", ControlFlowNodeRole::Exit, Some("NormalExit")),
                (
                    "exceptional-exit",
                    ControlFlowNodeRole::Exit,
                    Some("ExceptionalExit"),
                ),
            ],
            &[
                ("entry", "live", ControlFlowKind::Entry),
                ("live", "normal-exit", ControlFlowKind::Exit),
                ("dead", "normal-exit", ControlFlowKind::Exit),
            ],
        );

        let analysis = analyze_callable_dominance(&graph, callable_id()).expect("dominance");

        assert!(analysis.cfg.node_ids.contains(&test_id("dead")));
        assert!(!analysis.cfg.is_reachable(test_id("dead")));
        assert!(!analysis.dominators.node_ids.contains(&test_id("dead")));
        assert!(!analysis.post_dominators.node_ids.contains(&test_id("dead")));
        assert!(!analysis.dominates(test_id("dead"), test_id("normal-exit")));
        assert!(!analysis.post_dominates(test_id("normal-exit"), test_id("dead")));
    }

    #[test]
    fn sg061_derives_python_controls_from_post_dominance() {
        let graph = build_python_supergraph(&sg061_project("sample.py", true));

        assert_sg061_controls(&graph, true);
    }

    #[test]
    fn sg061_derives_typescript_controls_from_post_dominance() {
        let graph = build_typescript_supergraph(&sg061_project("sample.ts", false));

        assert_sg061_controls(&graph, false);
    }

    #[test]
    fn sg122_asserts_python_control_dependence_semantics() {
        let graph = build_python_supergraph(&sg061_project("sample.py", true));

        assert_sg122_control_dependence_semantics(&graph, true);
    }

    #[test]
    fn sg122_asserts_typescript_control_dependence_semantics() {
        let graph = build_typescript_supergraph(&sg061_project("sample.ts", false));

        assert_sg122_control_dependence_semantics(&graph, false);
    }

    fn assert_sg061_controls(graph: &ProgramSupergraph, python: bool) {
        let controls = control_edges(graph);

        let flag = cfg_condition_at(graph, span(13, 17));
        let success = call_site_at(graph, span(25, 34));
        let recover = call_site_at(graph, span(60, 69));
        let value_definition = data_flow_definition_at(graph, span(40, 49));
        let after_if = call_site_at(graph, span(95, 105));
        let if_merge = cfg_merge_at(graph, span(10, 90), "BranchMerge");

        assert_control(&controls, flag, success);
        assert_control(&controls, flag, recover);
        assert_control(&controls, flag, value_definition);
        assert_no_control(&controls, flag, after_if);
        assert_no_control(&controls, flag, if_merge);

        let keep = cfg_condition_at(graph, span(116, 120));
        let tick = call_site_at(graph, span(125, 131));
        let after_loop = call_site_at(graph, span(185, 197));
        assert_control(&controls, keep, tick);
        assert_no_control(&controls, keep, after_loop);

        let done = cfg_condition_at(graph, span(203, 207));
        let return_cfg = cfg_return_at(graph, span(215, 227));
        assert_control(&controls, done, return_cfg);

        let fatal = cfg_condition_at(graph, span(263, 268));
        let raise_cfg = cfg_raise_at(graph, span(275, 286));
        assert_control(&controls, fatal, raise_cfg);

        let nested_stop = cfg_condition_at(graph, span(143, 147));
        let break_statement = statement_fact_with_label(graph, "break");
        assert_control(&controls, nested_stop, break_statement);

        assert!(
            graph.edges.iter().all(|edge| match &edge.fact {
                EdgeFact::Controls(controls) => controls.precision == PRECISION,
                _ => true,
            }),
            "SG-061 should replace the old containment Controls approximation"
        );
        assert!(
            graph.edges.iter().any(|edge| matches!(
                &edge.fact,
                EdgeFact::Controls(controls)
                    if controls.callable_id == callable_id()
                        && edge.confidence == Confidence::Exact
                        && edge.uncertainty == Uncertainty::Exact
            )),
            "post-dominance Controls facts should be exact derived facts"
        );

        let branch_statement = if python {
            "if flag:\n    success()\n    value = 1\nelse:\n    recover()"
        } else {
            "if (flag) {\n  success();\n  value = 1;\n} else {\n  recover();\n}"
        };
        let branch_statement = statement_fact_with_label(graph, branch_statement);
        assert_no_control(&controls, flag, branch_statement);
    }

    fn assert_sg122_control_dependence_semantics(graph: &ProgramSupergraph, python: bool) {
        let controls = pdg_control_edges(graph);

        let flag = cfg_condition_at(graph, span(13, 17));
        let branch_statement = statement_fact_with_label(
            graph,
            if python {
                "if flag:\n    success()\n    value = 1\nelse:\n    recover()"
            } else {
                "if (flag) {\n  success();\n  value = 1;\n} else {\n  recover();\n}"
            },
        );
        let success_statement = statement_fact_with_label(graph, "success()");
        let success_call = call_site_at(graph, span(25, 34));
        let value_expression = expression_fact_at(graph, span(48, 49));
        let value_definition = definition_fact_at(graph, span(40, 49));
        let value_data_flow_definition = data_flow_definition_at(graph, span(40, 49));
        let recover_call = call_site_at(graph, span(60, 69));
        let if_merge = cfg_merge_at(graph, span(10, 90), "BranchMerge");
        let after_if = call_site_at(graph, span(95, 105));

        assert_control(&controls, flag, success_statement);
        assert_control(&controls, flag, success_call);
        assert_control(&controls, flag, value_expression);
        assert_control(&controls, flag, value_definition);
        assert_control(&controls, flag, value_data_flow_definition);
        assert_control(&controls, flag, recover_call);
        assert_no_control(&controls, flag, branch_statement);
        assert_no_control(&controls, flag, if_merge);
        assert_no_control(&controls, flag, after_if);

        let keep = cfg_condition_at(graph, span(116, 120));
        let tick_call = call_site_at(graph, span(125, 131));
        let nested_stop = cfg_condition_at(graph, span(143, 147));
        let after_loop = call_site_at(graph, span(185, 197));
        assert_control(&controls, keep, tick_call);
        assert_control(&controls, keep, nested_stop);
        assert_no_control(&controls, keep, after_loop);

        let break_statement = statement_fact_with_label(graph, "break");
        let continue_statement = statement_fact_with_label(graph, "continue");
        assert_control(&controls, nested_stop, break_statement);
        assert_control(&controls, nested_stop, continue_statement);

        let done = cfg_condition_at(graph, span(203, 207));
        let return_cfg = cfg_return_at(graph, span(215, 227));
        let after_done = call_site_at(graph, span(245, 257));
        assert_control(&controls, done, return_cfg);
        assert_control(&controls, done, after_done);

        let fatal = cfg_condition_at(graph, span(263, 268));
        let raise_cfg = cfg_raise_at(graph, span(275, 286));
        let after_fatal = call_site_at(graph, span(310, 323));
        assert_control(&controls, fatal, raise_cfg);
        assert_control(&controls, fatal, after_fatal);

        let pdg = graph.program_dependence_view();
        let call_slice = pdg.call_backward_slice(success_call);
        assert!(
            call_slice.control_condition_ids.contains(&flag),
            "call-site backward slice should include its controlling condition"
        );
        let definition_slice = pdg.write_backward_slice(value_definition);
        assert!(
            definition_slice.control_condition_ids.contains(&flag),
            "definition backward slice should include its controlling condition"
        );
        let after_if_slice = pdg.call_backward_slice(after_if);
        assert!(
            !after_if_slice.control_condition_ids.contains(&flag),
            "post-merge behavior should not inherit the branch condition"
        );

        assert!(
            graph.edges.iter().all(|edge| match &edge.fact {
                EdgeFact::Controls(controls) => controls.precision == PRECISION,
                _ => true,
            }),
            "SG-122 fixtures should only see post-dominance Controls facts"
        );
    }

    fn pdg_control_edges(graph: &ProgramSupergraph) -> BTreeSet<(NodeId, NodeId)> {
        let pdg = graph.program_dependence_view();
        let filter =
            ProgramDependenceGraphFilter::all_callables().with_callable(callable_id());
        pdg.edges(&filter)
            .filter_map(|edge| match &edge.fact {
                EdgeFact::Controls(_) => Some((edge.source_id, edge.target_id?)),
                _ => None,
            })
            .collect()
    }

    fn control_edges(graph: &ProgramSupergraph) -> BTreeSet<(NodeId, NodeId)> {
        graph
            .edges
            .iter()
            .filter_map(|edge| match &edge.fact {
                EdgeFact::Controls(_) => Some((edge.source_id, edge.target_id?)),
                _ => None,
            })
            .collect()
    }

    fn assert_control(edges: &BTreeSet<(NodeId, NodeId)>, condition_id: NodeId, target_id: NodeId) {
        assert!(
            edges.contains(&(condition_id, target_id)),
            "missing Controls edge {condition_id} -> {target_id}"
        );
    }

    fn assert_no_control(edges: &BTreeSet<(NodeId, NodeId)>, condition_id: NodeId, target_id: NodeId) {
        assert!(
            !edges.contains(&(condition_id, target_id)),
            "unexpected Controls edge {condition_id} -> {target_id}"
        );
    }

    fn cfg_condition_at(graph: &ProgramSupergraph, span: SourceSpan) -> NodeId {
        cfg_node_at(graph, span, ControlFlowNodeRole::Condition)
    }

    fn cfg_return_at(graph: &ProgramSupergraph, span: SourceSpan) -> NodeId {
        cfg_node_at(graph, span, ControlFlowNodeRole::Return)
    }

    fn cfg_raise_at(graph: &ProgramSupergraph, span: SourceSpan) -> NodeId {
        cfg_node_at(graph, span, ControlFlowNodeRole::Raise)
    }

    fn cfg_node_at(
        graph: &ProgramSupergraph,
        span: SourceSpan,
        role: ControlFlowNodeRole,
    ) -> NodeId {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::ControlFlow(control)
                    if node.span == Some(span) && control.role == role =>
                {
                    Some(control.cfg_node_id)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing CFG {role:?} at {span:?}"))
    }

    fn cfg_merge_at(graph: &ProgramSupergraph, span: SourceSpan, semantic_kind: &str) -> NodeId {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::ControlFlow(control)
                    if node.span == Some(span)
                        && control.role == ControlFlowNodeRole::Merge
                        && control.semantic_kind.as_deref() == Some(semantic_kind) =>
                {
                    Some(control.cfg_node_id)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing CFG merge {semantic_kind} at {span:?}"))
    }

    fn call_site_at(graph: &ProgramSupergraph, span: SourceSpan) -> NodeId {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::CallSite(call_site) if node.span == Some(span) => {
                    Some(call_site.call_site_id)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing call site at {span:?}"))
    }

    fn data_flow_definition_at(graph: &ProgramSupergraph, span: SourceSpan) -> NodeId {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::DataFlow(data_flow)
                    if node.span == Some(span)
                        && data_flow.role == sg::DataFlowNodeRole::Definition =>
                {
                    Some(data_flow.data_flow_node_id)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing definition at {span:?}"))
    }

    fn definition_fact_at(graph: &ProgramSupergraph, span: SourceSpan) -> NodeId {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Definition(definition) if node.span == Some(span) => {
                    Some(definition.definition_id)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing first-class definition at {span:?}"))
    }

    fn expression_fact_at(graph: &ProgramSupergraph, span: SourceSpan) -> NodeId {
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Expression(expression) if node.span == Some(span) => {
                    Some(expression.expression_id)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing expression at {span:?}"))
    }

    fn statement_fact_with_label(graph: &ProgramSupergraph, label: &str) -> NodeId {
        let statement_span = graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::ControlFlow(control)
                    if control.label == label && control.role == ControlFlowNodeRole::Statement =>
                {
                    node.span
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing CFG statement {label}"));
        graph
            .nodes
            .iter()
            .find_map(|node| match &node.fact {
                NodeFact::Statement(statement)
                    if statement.callable_id == callable_id()
                        && node.span == Some(statement_span) =>
                {
                    Some(statement.statement_id)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing statement fact {label}"))
    }

    fn sg061_project(path: &str, python: bool) -> ProjectAst {
        let branch_text = if python {
            "if flag:\n    success()\n    value = 1\nelse:\n    recover()"
        } else {
            "if (flag) {\n  success();\n  value = 1;\n} else {\n  recover();\n}"
        };
        let loop_text = if python {
            "while keep:\n    tick()\n    if stop:\n        break\n    continue"
        } else {
            "while (keep) {\n  tick();\n  if (stop) {\n    break;\n  }\n  continue;\n}"
        };
        let nested_text = if python {
            "if stop:\n        break"
        } else {
            "if (stop) {\n    break;\n  }"
        };
        let return_text = if python {
            "if done:\n    return value"
        } else {
            "if (done) {\n  return value;\n}"
        };
        let fatal_text = if python {
            "if fatal:\n    raise Fatal()"
        } else {
            "if (fatal) {\n  throw fatal;\n}"
        };
        ProjectAst {
            manifests: Vec::new(),
            root: String::new(),
            files: vec![FileAst {
                path: path.to_string(),
                imports: Vec::new(),
                assignments: Vec::new(),
                calls: vec![
                    call("success", span(25, 34)),
                    call("recover", span(60, 69)),
                    call("after_if", span(95, 105)),
                    call("tick", span(125, 131)),
                    call("after_loop", span(185, 197)),
                    call("after_done", span(245, 257)),
                    call("after_fatal", span(310, 323)),
                ],
                symbols: Vec::new(),
                statements: vec![
                    statement(AstStatementKind::If, branch_text, span(10, 90)),
                    statement(AstStatementKind::Expression, "success()", span(25, 34)),
                    statement(AstStatementKind::Assignment, "value = 1", span(40, 49)),
                    statement(AstStatementKind::Expression, "recover()", span(60, 69)),
                    statement(AstStatementKind::Expression, "after_if()", span(95, 105)),
                    statement(AstStatementKind::Loop, loop_text, span(110, 180)),
                    statement(AstStatementKind::Expression, "tick()", span(125, 131)),
                    statement(AstStatementKind::If, nested_text, span(140, 165)),
                    statement(AstStatementKind::Break, "break", span(150, 155)),
                    statement(AstStatementKind::Continue, "continue", span(170, 178)),
                    statement(AstStatementKind::Expression, "after_loop()", span(185, 197)),
                    statement(AstStatementKind::If, return_text, span(200, 240)),
                    statement(AstStatementKind::Return, "return value", span(215, 227)),
                    statement(AstStatementKind::Expression, "after_done()", span(245, 257)),
                    statement(AstStatementKind::If, fatal_text, span(260, 305)),
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
                        span(275, 286),
                    ),
                    statement(
                        AstStatementKind::Expression,
                        "after_fatal()",
                        span(310, 323),
                    ),
                ],
                expressions: vec![
                    expression(AstExpressionKind::Identifier, "flag", span(13, 17)),
                    expression(AstExpressionKind::Identifier, "keep", span(116, 120)),
                    expression(AstExpressionKind::Identifier, "stop", span(143, 147)),
                    expression(AstExpressionKind::Identifier, "done", span(203, 207)),
                    expression(AstExpressionKind::Identifier, "fatal", span(263, 268)),
                    expression(AstExpressionKind::Literal, "1", span(48, 49)),
                ],
                conditions: vec![
                    condition(AstConditionKind::If, "flag", span(13, 17)),
                    condition(AstConditionKind::While, "keep", span(116, 120)),
                    condition(AstConditionKind::If, "stop", span(143, 147)),
                    condition(AstConditionKind::If, "done", span(203, 207)),
                    condition(AstConditionKind::If, "fatal", span(263, 268)),
                ],
                definitions: vec![DefinitionAst {
                    name: "value".to_string(),
                    kind: DefinitionKind::Variable,
                    text: "value = 1".to_string(),
                    owner_id: CALLABLE_ID.to_string(),
                    source_span: span(40, 49),
                }],
                uses: Vec::new(),
                returns: vec![ReturnAst {
                    value: Some("value".to_string()),
                    owner_id: CALLABLE_ID.to_string(),
                    source_span: span(215, 227),
                }],
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
                    owner_id: CALLABLE_ID.to_string(),
                    source_span: span(275, 286),
                }],
                field_accesses: Vec::new(),
                index_accesses: Vec::new(),
                parse_errors: Vec::new(),
            }],
        }
    }

    fn statement(kind: AstStatementKind, text: &str, source_span: SourceSpan) -> StatementAst {
        StatementAst {
            kind,
            text: text.to_string(),
            owner_id: CALLABLE_ID.to_string(),
            source_span,
        }
    }

    fn expression(kind: AstExpressionKind, text: &str, source_span: SourceSpan) -> ExpressionAst {
        ExpressionAst {
            kind,
            text: text.to_string(),
            owner_id: CALLABLE_ID.to_string(),
            source_span,
        }
    }

    fn condition(kind: AstConditionKind, text: &str, source_span: SourceSpan) -> ConditionAst {
        ConditionAst {
            kind,
            text: text.to_string(),
            owner_id: CALLABLE_ID.to_string(),
            source_span,
        }
    }

    fn call(callee: &str, source_span: SourceSpan) -> CallAst {
        CallAst {
            callee: callee.to_string(),
            receiver: None,
            argument_names: Vec::new(),
            args_count: 0,
            context: CallContext::ModuleInitializer,
            source_span,
        }
    }

    fn span(start_byte: u32, end_byte: u32) -> SourceSpan {
        SourceSpan {
            start_byte,
            end_byte,
            start_row: 0,
            start_column: start_byte as u32,
            end_row: 0,
            end_column: end_byte as u32,
        }
    }

    fn graph_fixture(
        nodes: &[(&str, ControlFlowNodeRole, Option<&str>)],
        edges: &[(&str, &str, ControlFlowKind)],
    ) -> ProgramSupergraph {
        let nodes = nodes
            .iter()
            .map(|(node_id, role, semantic_kind)| cfg_node(node_id, *role, *semantic_kind))
            .collect::<Vec<_>>();
        let edges = edges
            .iter()
            .map(|(source, target, kind)| cfg_edge(source, target, *kind))
            .collect::<Vec<_>>();
        let indexes = build_indexes(&nodes, &edges);
        ProgramSupergraph {
            schema_version: SCHEMA_VERSION.to_string(),
            language: "python".to_string(),
            root: String::new(),
            nodes,
            edges,
            indexes,
            id_cache: Default::default(),
        }
    }

    fn cfg_node(
        node_id: &str,
        role: ControlFlowNodeRole,
        semantic_kind: Option<&str>,
    ) -> GraphNode {
        GraphNode {
            node_id: test_id(node_id),
            fact_id: None,
            payload_hash: None,
            kind: NodeKind::ControlFlow,
            owner: owner(),
            span: None,
            confidence: Confidence::Exact,
            uncertainty: Uncertainty::Exact,
            evidence: Vec::new(),
            fact: NodeFact::ControlFlow(sg::ControlFlowNode {
                cfg_node_id: test_id(node_id),
                callable_id: callable_id(),
                role,
                label: Sym::from(node_id.to_string()),
                semantic_kind: (semantic_kind.map(str::to_string)).map(Sym::from),
            }),
        }
    }

    fn cfg_edge(source: &str, target: &str, flow_kind: ControlFlowKind) -> GraphEdge {
        GraphEdge {
            edge_id: test_edge_id(&format!("{source}->{target}")),
            fact_id: None,
            payload_hash: None,
            kind: EdgeKind::ControlFlow,
            source_id: test_id(source),
            target_id: Some(test_id(target)),
            owner: owner(),
            span: None,
            confidence: Confidence::Exact,
            uncertainty: Uncertainty::Exact,
            evidence: vec![sg::Evidence {
                kind: EvidenceKind::Inference,
                summary: Sym::from("SG-060 CFG fixture".to_string()),
                source_id: None,
                source_span: None,
                content_hash: None,
                syntax: None,
            }],
            fact: EdgeFact::ControlFlow(sg::ControlFlow {
                callable_id: callable_id(),
                flow_kind,
                outcome: ControlFlowOutcome::Unknown,
                branch_arm: None,
                precision: Sym::from("sg060-test-fixture".to_string()),
            }),
        }
    }

    fn owner() -> SourceOwnership {
        SourceOwnership {
            artifact_id: Some(test_id("sample.py")),
            scope_id: None,
            callable_id: Some(callable_id()),
        }
    }
}
