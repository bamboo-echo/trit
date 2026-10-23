//! 三值拓扑：边带 `True` / `Unknown` / `False` 的有向图可达性。
//!
//! 路径值取边值的最小值（`False < Unknown < True`），同一对节点的可达性取所有路径的
//! 最大值：纯 `True` 路径 → `True`，只有含 `Unknown` 边的路径 → `Unknown`，
//! 没有可用路径 → `False`。`False` 边视为断开。
//!
//! 传播是格上的 max-min 闭包。因为格只有 3 层，`True` 一旦成立就等价于
//! 「只走 `True` 边可达」，`Unknown` 等价于「从 `True` 可达集出发、只走非 `False` 边可达」，
//! 所以可以用两趟 BFS 得到**同一个最小不动点**：第一趟 `O(V+E)` 求 `True` 集，
//! 第二趟从 `True` 集出发求 `Unknown` 集。旧的不动点写法是对每条边反复松弛，
//! 最坏 `O(V·E)`，而且不给出「凭什么」。
//!
//! [`TernaryTopology::explain`] 在三值结果之外给出证书：`True` 给纯 `True` 路径，
//! `Unknown` 给一条路径以及路径上需要补全的 `Unknown` 边。这就是三值自己的验证结论——
//! **通过 / 未定 / 否定**，其中「未定」带可执行的补全线索，而不是二值的「可能」。

use std::collections::VecDeque;

use crate::value::Ternary;

/// 三值拓扑图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TernaryTopology {
    nodes: usize,
    edges: Vec<TernaryEdge>,
    adjacency: Vec<Vec<(usize, Ternary)>>,
}

/// 一条三值边。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TernaryEdge {
    /// 起点。
    pub from: usize,
    /// 终点。
    pub to: usize,
    /// 边的三值强度。
    pub value: Ternary,
}

/// 三值验证结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 命题在所有补全下成立（有纯 `True` 证书）。
    Proved,
    /// 只有把路径上的 `Unknown` 边补成存在才成立。
    Undetermined,
    /// 命题不成立（没有任何可用路径）。
    Refuted,
}

/// 一次可达性判定的结论与证书。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReachEvidence {
    /// 三值结论。
    pub verdict: Verdict,
    /// 三值可达性：与 [`TernaryTopology::reachability`] 对应位置的值一致。
    pub value: Ternary,
    /// 证书路径；起点等于终点时就是 `[origin]`，`Refuted` 时为空。
    pub path: Vec<usize>,
    /// 路径上取值为 `Unknown` 的边（需要补全成存在才能让结论升级为 `Proved`）。
    pub unknown_edges: Vec<(usize, usize)>,
}

impl TernaryTopology {
    /// 建一个含 `nodes` 个节点的空图。
    pub fn new(nodes: usize) -> Self {
        Self {
            nodes,
            edges: Vec::new(),
            adjacency: vec![Vec::new(); nodes],
        }
    }

    /// 节点数。
    pub fn nodes(&self) -> usize {
        self.nodes
    }

    /// 边。
    pub fn edges(&self) -> &[TernaryEdge] {
        &self.edges
    }

    /// 加一条边；端点越界时报错。`False` 边表示断开，不进入图。
    pub fn add_edge(
        &mut self,
        from: usize,
        to: usize,
        value: Ternary,
    ) -> Result<(), TopologyError> {
        self.check(from)?;
        self.check(to)?;
        if value != Ternary::False {
            self.edges.push(TernaryEdge { from, to, value });
            self.adjacency[from].push((to, value));
        }
        Ok(())
    }

    /// 从 `origin` 出发到每个节点的三值可达性。
    ///
    /// 返回长度等于节点数的向量；`origin` 自身为 `True`。
    pub fn reachability(&self, origin: usize) -> Result<Vec<Ternary>, TopologyError> {
        self.check(origin)?;
        let adjacency = &self.adjacency;
        let mut best = vec![Ternary::False; self.nodes];
        best[origin] = Ternary::True;
        let mut queue = VecDeque::from([origin]);
        while let Some(node) = queue.pop_front() {
            for &(next, value) in &adjacency[node] {
                if value == Ternary::True && best[next] == Ternary::False {
                    best[next] = Ternary::True;
                    queue.push_back(next);
                }
            }
        }
        let mut queue: VecDeque<usize> = (0..self.nodes)
            .filter(|&node| best[node] == Ternary::True)
            .collect();
        while let Some(node) = queue.pop_front() {
            for &(next, value) in &adjacency[node] {
                if value != Ternary::False && best[next] == Ternary::False {
                    best[next] = Ternary::Unknown;
                    queue.push_back(next);
                }
            }
        }
        Ok(best)
    }

    /// 判定「`target` 从 `origin` 可达」并给出证书。
    ///
    /// 三值结论而非二值：`Proved` 表示所有补全下都可达；`Undetermined` 表示只有把
    /// 路径上的 `Unknown` 边补成存在才可达，证书里列出这些边；`Refuted` 表示不可达。
    pub fn explain(&self, origin: usize, target: usize) -> Result<ReachEvidence, TopologyError> {
        self.check(origin)?;
        self.check(target)?;
        let adjacency = &self.adjacency;

        let mut strong = vec![false; self.nodes];
        let mut strong_pred: Vec<Option<usize>> = vec![None; self.nodes];
        strong[origin] = true;
        let mut queue = VecDeque::from([origin]);
        while let Some(node) = queue.pop_front() {
            for &(next, value) in &adjacency[node] {
                if value == Ternary::True && !strong[next] {
                    strong[next] = true;
                    strong_pred[next] = Some(node);
                    queue.push_back(next);
                }
            }
        }
        if strong[target] {
            return Ok(ReachEvidence {
                verdict: Verdict::Proved,
                value: Ternary::True,
                path: reconstruct(&strong_pred, origin, target),
                unknown_edges: Vec::new(),
            });
        }

        let mut weak_pred: Vec<Option<usize>> = vec![None; self.nodes];
        let mut weak_via: Vec<Option<Ternary>> = vec![None; self.nodes];
        let mut weak = vec![false; self.nodes];
        let mut queue: VecDeque<usize> = (0..self.nodes).filter(|&node| strong[node]).collect();
        for &node in &queue {
            weak[node] = true;
        }
        while let Some(node) = queue.pop_front() {
            for &(next, value) in &adjacency[node] {
                if !weak[next] {
                    weak[next] = true;
                    weak_pred[next] = Some(node);
                    weak_via[next] = Some(value);
                    queue.push_back(next);
                }
            }
        }
        if !weak[target] {
            return Ok(ReachEvidence {
                verdict: Verdict::Refuted,
                value: Ternary::False,
                path: Vec::new(),
                unknown_edges: Vec::new(),
            });
        }

        // 从 target 沿 weak 前驱回溯到某个 strong 种子，再把种子到 origin 的强路径接上。
        let mut suffix = vec![target];
        let mut current = target;
        while let Some(previous) = weak_pred[current] {
            suffix.push(previous);
            current = previous;
        }
        suffix.reverse();
        let seed = suffix[0];
        let mut path = reconstruct(&strong_pred, origin, seed);
        path.pop();
        path.extend(suffix);
        let unknown_edges = path
            .iter()
            .skip(1)
            .filter(|&&node| weak_via[node] == Some(Ternary::Unknown))
            .map(|&node| (weak_pred[node].expect("弱路径节点必有前驱"), node))
            .collect();
        Ok(ReachEvidence {
            verdict: Verdict::Undetermined,
            value: Ternary::Unknown,
            path,
            unknown_edges,
        })
    }

    fn check(&self, node: usize) -> Result<(), TopologyError> {
        if node < self.nodes {
            Ok(())
        } else {
            Err(TopologyError::NodeOutOfRange {
                node,
                nodes: self.nodes,
            })
        }
    }
}

fn reconstruct(pred: &[Option<usize>], origin: usize, target: usize) -> Vec<usize> {
    if origin == target {
        return vec![origin];
    }
    let mut path = vec![target];
    let mut current = target;
    while let Some(previous) = pred[current] {
        path.push(previous);
        current = previous;
        if previous == origin {
            path.reverse();
            return path;
        }
    }
    Vec::new()
}

/// 三值拓扑错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TopologyError {
    /// 节点编号越界。
    #[error("节点 {node} 越界（共 {nodes} 个节点）")]
    NodeOutOfRange {
        /// 越界节点。
        node: usize,
        /// 节点数。
        nodes: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn true_chain_reaches_everything() {
        let mut graph = TernaryTopology::new(3);
        graph.add_edge(0, 1, Ternary::True).unwrap();
        graph.add_edge(1, 2, Ternary::True).unwrap();

        assert_eq!(
            graph.reachability(0).unwrap(),
            vec![Ternary::True, Ternary::True, Ternary::True]
        );
        assert_eq!(
            graph.reachability(1).unwrap(),
            vec![Ternary::False, Ternary::True, Ternary::True]
        );
    }

    #[test]
    fn unknown_edge_propagates_unknown_until_true_path_exists() {
        let mut graph = TernaryTopology::new(3);
        graph.add_edge(0, 1, Ternary::Unknown).unwrap();
        graph.add_edge(1, 2, Ternary::True).unwrap();
        assert_eq!(
            graph.reachability(0).unwrap(),
            vec![Ternary::True, Ternary::Unknown, Ternary::Unknown]
        );
        graph.add_edge(0, 2, Ternary::True).unwrap();
        assert_eq!(
            graph.reachability(0).unwrap(),
            vec![Ternary::True, Ternary::Unknown, Ternary::True]
        );
    }

    #[test]
    fn false_and_cycles_are_handled() {
        let mut graph = TernaryTopology::new(3);
        graph.add_edge(0, 1, Ternary::False).unwrap();
        graph.add_edge(0, 2, Ternary::Unknown).unwrap();
        graph.add_edge(2, 0, Ternary::Unknown).unwrap();
        assert_eq!(graph.edges().len(), 2, "False 边不进入图");
        assert_eq!(
            graph.reachability(0).unwrap(),
            vec![Ternary::True, Ternary::False, Ternary::Unknown]
        );
        assert!(matches!(
            graph.reachability(3),
            Err(TopologyError::NodeOutOfRange { .. })
        ));
        assert!(matches!(
            graph.add_edge(0, 3, Ternary::True),
            Err(TopologyError::NodeOutOfRange { .. })
        ));
    }

    /// 旧实现（逐边松弛到不动点）作为参考口径，用来对拍两趟 BFS。
    fn reference_reachability(graph: &TernaryTopology, origin: usize) -> Vec<Ternary> {
        let mut best = vec![Ternary::False; graph.nodes()];
        best[origin] = Ternary::True;
        loop {
            let mut changed = false;
            for edge in graph.edges() {
                let source = best[edge.from];
                if source == Ternary::False {
                    continue;
                }
                let path = source.min(edge.value);
                if path > best[edge.to] {
                    best[edge.to] = path;
                    changed = true;
                }
            }
            if !changed {
                return best;
            }
        }
    }

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    #[test]
    fn bfs_matches_fixed_point_on_random_graphs() {
        for seed in 1..=200u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let nodes = 2 + (rng.next() as usize % 12);
            let mut graph = TernaryTopology::new(nodes);
            let edges = rng.next() as usize % (nodes * 3);
            for _ in 0..edges {
                let from = rng.next() as usize % nodes;
                let to = rng.next() as usize % nodes;
                let value = match rng.next() % 3 {
                    0 => Ternary::False,
                    1 => Ternary::Unknown,
                    _ => Ternary::True,
                };
                graph.add_edge(from, to, value).unwrap();
            }
            for origin in 0..nodes {
                assert_eq!(
                    graph.reachability(origin).unwrap(),
                    reference_reachability(&graph, origin),
                    "seed={seed} origin={origin} edges={:?}",
                    graph.edges()
                );
            }
        }
    }

    #[test]
    fn explain_proves_with_true_only_path() {
        let mut graph = TernaryTopology::new(3);
        graph.add_edge(0, 1, Ternary::Unknown).unwrap();
        graph.add_edge(0, 2, Ternary::True).unwrap();
        graph.add_edge(2, 1, Ternary::True).unwrap();

        let evidence = graph.explain(0, 1).unwrap();
        assert_eq!(evidence.verdict, Verdict::Proved);
        assert_eq!(evidence.value, Ternary::True);
        assert_eq!(evidence.path, vec![0, 2, 1]);
        assert!(evidence.unknown_edges.is_empty());
    }

    #[test]
    fn explain_reports_unknown_edges_for_contingent_path() {
        let mut graph = TernaryTopology::new(4);
        graph.add_edge(0, 1, Ternary::True).unwrap();
        graph.add_edge(1, 2, Ternary::Unknown).unwrap();
        graph.add_edge(2, 3, Ternary::Unknown).unwrap();

        let evidence = graph.explain(0, 3).unwrap();
        assert_eq!(evidence.verdict, Verdict::Undetermined);
        assert_eq!(evidence.value, Ternary::Unknown);
        assert_eq!(evidence.path, vec![0, 1, 2, 3]);
        assert_eq!(evidence.unknown_edges, vec![(1, 2), (2, 3)]);
    }

    #[test]
    fn explain_refutes_when_every_path_is_broken() {
        let mut graph = TernaryTopology::new(4);
        graph.add_edge(0, 1, Ternary::True).unwrap();
        graph.add_edge(1, 2, Ternary::False).unwrap();
        graph.add_edge(0, 3, Ternary::Unknown).unwrap();
        graph.add_edge(3, 2, Ternary::False).unwrap();

        let evidence = graph.explain(0, 2).unwrap();
        assert_eq!(evidence.verdict, Verdict::Refuted);
        assert_eq!(evidence.value, Ternary::False);
        assert!(evidence.path.is_empty());
    }

    #[test]
    fn explain_origin_reaches_itself_without_edges() {
        let graph = TernaryTopology::new(2);
        let evidence = graph.explain(1, 1).unwrap();
        assert_eq!(evidence.verdict, Verdict::Proved);
        assert_eq!(evidence.path, vec![1]);
    }

    #[test]
    fn explain_agrees_with_reachability_on_random_graphs() {
        for seed in 1..=100u64 {
            let mut rng = Rng(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1);
            let nodes = 2 + (rng.next() as usize % 10);
            let mut graph = TernaryTopology::new(nodes);
            let edges = rng.next() as usize % (nodes * 3);
            for _ in 0..edges {
                let from = rng.next() as usize % nodes;
                let to = rng.next() as usize % nodes;
                let value = match rng.next() % 3 {
                    0 => Ternary::False,
                    1 => Ternary::Unknown,
                    _ => Ternary::True,
                };
                graph.add_edge(from, to, value).unwrap();
            }
            for origin in 0..nodes {
                let values = graph.reachability(origin).unwrap();
                for (target, expected) in values.iter().enumerate() {
                    let evidence = graph.explain(origin, target).unwrap();
                    assert_eq!(&evidence.value, expected, "seed={seed} {origin}->{target}");
                    match evidence.verdict {
                        Verdict::Proved => assert_eq!(evidence.value, Ternary::True),
                        Verdict::Undetermined => {
                            assert_eq!(evidence.value, Ternary::Unknown);
                            assert!(!evidence.unknown_edges.is_empty());
                        }
                        Verdict::Refuted => assert_eq!(evidence.value, Ternary::False),
                    }
                }
            }
        }
    }
}
