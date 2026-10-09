//! Acceptance benchmark: live/peak heap after analysis plus per-item sizes.
//! Usage: cargo run --release --example memprobe -- <rust-path>
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            let live = LIVE.fetch_add(l.size(), Relaxed) + l.size();
            PEAK.fetch_max(live, Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        LIVE.fetch_sub(l.size(), Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, new) };
        if !q.is_null() {
            if new >= l.size() {
                let live = LIVE.fetch_add(new - l.size(), Relaxed) + (new - l.size());
                PEAK.fetch_max(live, Relaxed);
            } else {
                LIVE.fetch_sub(l.size() - new, Relaxed);
            }
        }
        q
    }
}

#[global_allocator]
static A: Counting = Counting;

macro_rules! for_each_index {
    ($graph:expr, [$($f:ident),*]) => {
        $(
            let live = LIVE.load(Relaxed);
            drop(std::mem::take(&mut $graph.indexes.$f));
            println!("  idx {:<36} {:>8.1} MB", stringify!($f), mb(live - LIVE.load(Relaxed)));
        )*
    };
}
fn mb(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("path argument");
    let before = LIVE.load(Relaxed);
    let mut graph = supergraph::analyze_rust_supergraph(std::path::Path::new(&path))?;
    let retained = LIVE.load(Relaxed) - before;
    println!("peak     {:>10.1} MB", mb(PEAK.load(Relaxed)));
    println!("retained {:>10.1} MB", mb(retained));
    println!("nodes {} edges {}", graph.nodes.len(), graph.edges.len());
    println!(
        "size_of GraphNode {} GraphEdge {} Evidence {}",
        std::mem::size_of::<supergraph::supergraph::GraphNode>(),
        std::mem::size_of::<supergraph::supergraph::GraphEdge>(),
        std::mem::size_of::<supergraph::supergraph::Evidence>(),
    );
    {
        let mut graph = graph.clone();
    for_each_index!(graph, [node_position_by_id, edge_position_by_id, nodes_by_kind, edges_by_kind, nodes_by_uncertainty, edges_by_uncertainty, outgoing_edges_by_node, incoming_edges_by_node, source_span_to_nodes, artifact_to_nodes, callable_to_nodes, owner_to_nodes, owner_to_edges, symbol_to_definitions, symbol_to_uses, requirement_to_code, code_to_requirements, requirement_to_domain_knowledge, domain_knowledge_to_requirements, calls_by_caller, calls_by_concrete_target, call_site_to_calls, caller_to_concrete_target_calls, caller_to_concrete_call_targets]);
    }
    let live = LIVE.load(Relaxed);
    let indexes = std::mem::take(&mut graph.indexes);
    drop(indexes);
    println!("indexes  {:>10.1} MB", mb(live - LIVE.load(Relaxed)));
    let live = LIVE.load(Relaxed);
    graph.drop_id_cache();
    println!("id_cache {:>10.1} MB", mb(live - LIVE.load(Relaxed)));
    println!(
        "nodes vec {:.1} MB, edges vec {:.1} MB (inline), interned strings {}",
        mb(graph.nodes.capacity() * std::mem::size_of::<supergraph::supergraph::GraphNode>()),
        mb(graph.edges.capacity() * std::mem::size_of::<supergraph::supergraph::GraphEdge>()),
        supergraph::intern::Sym::count(),
    );
    let mut by_kind: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    for node in &graph.nodes {
        let b = LIVE.load(Relaxed);
        let copy = node.clone();
        let heap = LIVE.load(Relaxed) - b;
        std::mem::forget(copy);
        LIVE.fetch_sub(heap, Relaxed);
        let e = by_kind.entry(format!("node {:?}", node.kind)).or_default();
        e.0 += 1;
        e.1 += heap;
    }
    for edge in &graph.edges {
        let b = LIVE.load(Relaxed);
        let copy = edge.clone();
        let heap = LIVE.load(Relaxed) - b;
        std::mem::forget(copy);
        LIVE.fetch_sub(heap, Relaxed);
        let e = by_kind.entry(format!("edge {:?}", edge.kind)).or_default();
        e.0 += 1;
        e.1 += heap;
    }
    for (kind, (n, heap)) in by_kind {
        println!("{kind:<28} {n:>8} items {:>9.1} MB heap ({} B/item)", mb(heap), heap / n.max(1));
    }
    Ok(())
}
