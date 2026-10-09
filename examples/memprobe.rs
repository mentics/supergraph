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

fn mb(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("path argument");
    let before = LIVE.load(Relaxed);
    let graph = supergraph::analyze_rust_supergraph(std::path::Path::new(&path))?;
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
