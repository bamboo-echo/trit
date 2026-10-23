//! 三值拓扑：边带 `True` / `Unknown` / `False` 的有向图可达性。
//!
//! 路径值取边值的最小值（`False < Unknown < True`），同一对节点的可达性取所有路径的
//! 最大值：纯 `True` 路径 → `True`，只有含 `Unknown` 边的路径 → `Unknown`，
//! 没有可用路径 → `False`。`False` 边视为断开。

use crate::value::Ternary;

/// 三值拓扑图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TernaryTopology {
    nodes: usize,
    edges: Vec<TernaryEdge>,
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

impl TernaryTopology {
    /// 建一个含 `nodes` 个节点的空图。
    pub fn new(nodes: usize) -> Self {
        Self {
            nodes,
            edges: Vec::new(),
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

    /// 加一条边；端点越界时报错。
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
        }
        Ok(())
    }

    /// 从 `origin` 出发到每个节点的三值可达性。
    ///
    /// 返回长度等于节点数的向量；`origin` 自身为 `True`。
    pub fn reachability(&self, origin: usize) -> Result<Vec<Ternary>, TopologyError> {
        self.check(origin)?;
        let mut best = vec![Ternary::False; self.nodes];
        best[origin] = Ternary::True;
        loop {
            let mut changed = false;
            for edge in &self.edges {
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
                return Ok(best);
            }
        }
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
}
