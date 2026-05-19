//! Pure DAG topology primitives — generic over node identifiers.
//!
//! Operates on arbitrary `Hash + Eq + Clone` node IDs so we can reuse it for
//! P-item plans today and any other graph-shaped data later. See
//! `crate::plan::DagPlan` for the P-item-specific wrapper.
//!
//! Convention used throughout this module: `deps_of(node)` returns the *direct
//! dependencies* of `node` — i.e. nodes that must be ready before `node` itself
//! can run. References to unknown nodes (not in `nodes`) are silently dropped;
//! the caller is expected to run `validate_refs` separately if it cares.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// Topological sort using Kahn's algorithm.
///
/// On success returns an ordering where every dep precedes its dependants.
/// On failure (the graph has a cycle) returns the witness from
/// `find_cycle`.
pub fn topo_sort<N, F>(nodes: &[N], deps_of: F) -> Result<Vec<N>, Vec<N>>
where
    N: Clone + Eq + Hash,
    F: Fn(&N) -> Vec<N>,
{
    let known: HashSet<N> = nodes.iter().cloned().collect();
    let mut indegree: HashMap<N, usize> = nodes.iter().map(|n| (n.clone(), 0)).collect();
    let mut dependants: HashMap<N, Vec<N>> = HashMap::new();

    for n in nodes {
        for d in deps_of(n) {
            if !known.contains(&d) {
                continue;
            }
            *indegree.get_mut(n).expect("indegree initialised for every node") += 1;
            dependants.entry(d).or_default().push(n.clone());
        }
    }

    let mut ready: Vec<N> = indegree
        .iter()
        .filter_map(|(n, &d)| if d == 0 { Some(n.clone()) } else { None })
        .collect();
    let mut order = Vec::with_capacity(nodes.len());
    while let Some(n) = ready.pop() {
        order.push(n.clone());
        if let Some(children) = dependants.get(&n) {
            for c in children {
                let entry = indegree
                    .get_mut(c)
                    .expect("dependant must be in indegree map");
                *entry -= 1;
                if *entry == 0 {
                    ready.push(c.clone());
                }
            }
        }
    }

    if order.len() == nodes.len() {
        Ok(order)
    } else {
        Err(find_cycle(nodes, deps_of).unwrap_or_default())
    }
}

/// `Some(path)` if a cycle exists, where `path` walks one cycle witness with
/// the closing node appended (so `path[0] == path[last]`). `None` if acyclic.
pub fn find_cycle<N, F>(nodes: &[N], deps_of: F) -> Option<Vec<N>>
where
    N: Clone + Eq + Hash,
    F: Fn(&N) -> Vec<N>,
{
    let known: HashSet<N> = nodes.iter().cloned().collect();
    let mut state: HashMap<N, u8> = HashMap::new(); // 0=new, 1=visiting, 2=done
    let mut path: Vec<N> = Vec::new();
    for start in nodes {
        if state.get(start).copied().unwrap_or(0) == 2 {
            continue;
        }
        if let Some(cycle) = dfs(start, &deps_of, &known, &mut state, &mut path) {
            return Some(cycle);
        }
    }
    None
}

/// Convenience: `true` iff the graph has a cycle.
pub fn has_cycle<N, F>(nodes: &[N], deps_of: F) -> bool
where
    N: Clone + Eq + Hash,
    F: Fn(&N) -> Vec<N>,
{
    find_cycle(nodes, deps_of).is_some()
}

fn dfs<N, F>(
    n: &N,
    deps_of: &F,
    known: &HashSet<N>,
    state: &mut HashMap<N, u8>,
    path: &mut Vec<N>,
) -> Option<Vec<N>>
where
    N: Clone + Eq + Hash,
    F: Fn(&N) -> Vec<N>,
{
    match state.get(n).copied().unwrap_or(0) {
        1 => {
            let start = path.iter().position(|x| x == n).unwrap_or(0);
            let mut cycle: Vec<N> = path[start..].to_vec();
            cycle.push(n.clone());
            return Some(cycle);
        }
        2 => return None,
        _ => {}
    }
    state.insert(n.clone(), 1);
    path.push(n.clone());
    for d in deps_of(n) {
        if !known.contains(&d) {
            continue;
        }
        if let Some(c) = dfs(&d, deps_of, known, state, path) {
            return Some(c);
        }
    }
    path.pop();
    state.insert(n.clone(), 2);
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn graph(edges: &[(&str, &[&str])]) -> (Vec<String>, HashMap<String, Vec<String>>) {
        let nodes: Vec<String> = edges.iter().map(|(n, _)| (*n).to_string()).collect();
        let deps: HashMap<String, Vec<String>> = edges
            .iter()
            .map(|(n, ds)| (n.to_string(), ds.iter().map(|s| s.to_string()).collect()))
            .collect();
        (nodes, deps)
    }

    #[test]
    fn topo_sort_linear() {
        let (nodes, deps) = graph(&[
            ("a", &[]),
            ("b", &["a"]),
            ("c", &["b"]),
        ]);
        let order = topo_sort(&nodes, |n| deps[n].clone()).unwrap();
        let pos = |x: &str| order.iter().position(|s| s == x).unwrap();
        assert!(pos("a") < pos("b"));
        assert!(pos("b") < pos("c"));
    }

    #[test]
    fn topo_sort_fan_in_out() {
        // A → B → D
        //   ↘ C ↗
        let (nodes, deps) = graph(&[
            ("a", &[]),
            ("b", &["a"]),
            ("c", &["a"]),
            ("d", &["b", "c"]),
        ]);
        let order = topo_sort(&nodes, |n| deps[n].clone()).unwrap();
        let pos = |x: &str| order.iter().position(|s| s == x).unwrap();
        assert!(pos("a") < pos("b"));
        assert!(pos("a") < pos("c"));
        assert!(pos("b") < pos("d"));
        assert!(pos("c") < pos("d"));
    }

    #[test]
    fn detects_simple_cycle() {
        let (nodes, deps) = graph(&[
            ("a", &["b"]),
            ("b", &["a"]),
        ]);
        let cycle = find_cycle(&nodes, |n| deps[n].clone()).expect("cycle");
        // Cycle must close on itself.
        assert_eq!(cycle.first(), cycle.last());
        let set: HashSet<&String> = cycle.iter().collect();
        assert!(set.contains(&"a".to_string()) && set.contains(&"b".to_string()));
    }

    #[test]
    fn detects_self_loop() {
        let (nodes, deps) = graph(&[("a", &["a"])]);
        assert!(has_cycle(&nodes, |n| deps[n].clone()));
    }

    #[test]
    fn topo_sort_returns_err_on_cycle() {
        let (nodes, deps) = graph(&[
            ("a", &["c"]),
            ("b", &["a"]),
            ("c", &["b"]),
        ]);
        let err = topo_sort(&nodes, |n| deps[n].clone()).unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn unknown_deps_are_silently_ignored() {
        // "b" depends on "ghost" which doesn't exist — treat as no edge.
        let (nodes, deps) = graph(&[
            ("a", &[]),
            ("b", &["ghost"]),
        ]);
        let order = topo_sort(&nodes, |n| deps[n].clone()).unwrap();
        assert_eq!(order.len(), 2);
    }

    #[test]
    fn empty_graph() {
        let nodes: Vec<String> = vec![];
        let order = topo_sort(&nodes, |_| Vec::<String>::new()).unwrap();
        assert!(order.is_empty());
    }
}
