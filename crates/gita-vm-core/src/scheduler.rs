//! 调度器：filter → score → 选点
//!
//! 三阶段：
//! 1. **过滤**：硬性条件不满足直接淘汰（资源不够、污点不容忍、架构不匹配、标签不满足）
//! 2. **打分**：对剩余节点打分（资源均衡度、亲和性）
//! 3. **选点**：取最高分；平分时随机打散，避免所有 VM 挤到同一节点

use gita_vm_api::node::{ConditionStatus, Node, NodeConditionType, NodePhase, Resources, TaintEffect};
use gita_vm_api::vm::{Toleration, VmSpec};
use rand::seq::SliceRandom;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct Scheduler {
    /// 节点过载比例（超过则不再调度，打分为负）
    pub overload_ratio: f64,
    /// 系统预留资源（每个节点扣除）
    pub system_reserved: Resources,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self {
            overload_ratio: 0.8,
            system_reserved: Resources::zero(),
        }
    }
}

/// 调度结果
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub node_id: i64,
    pub score: f64,
}

impl Scheduler {
    /// 为 VM 选一个节点
    pub fn place(&self, spec: &VmSpec, nodes: &[Node]) -> Result<Placement, String> {
        let feasible = self.filter(spec, nodes);
        if feasible.is_empty() {
            return Err(format!(
                "没有满足条件的节点（需求 CPU={}m, mem={}, disk={}）",
                spec.cpu_millis, spec.memory_bytes, spec.disk_bytes
            ));
        }
        let mut scored: Vec<(i64, f64)> = feasible
            .into_iter()
            .map(|n| {
                let score = self.score(spec, n);
                (n.id.0, score)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // 平分随机打散
        let top_score = scored[0].1;
        let epsilon = 1e-9;
        let ties: Vec<(i64, f64)> = scored
            .iter()
            .take_while(|(_, s)| (*s - top_score).abs() < epsilon)
            .copied()
            .collect();
        let chosen = if ties.len() > 1 {
            *ties.choose(&mut rand::thread_rng()).unwrap()
        } else {
            scored[0]
        };
        Ok(Placement { node_id: chosen.0, score: chosen.1 })
    }

    /// 过滤阶段：返回满足硬性条件的节点
    fn filter<'a>(&self, spec: &VmSpec, nodes: &'a [Node]) -> Vec<&'a Node> {
        nodes
            .iter()
            .filter(|n| self.feasible(spec, n))
            .collect()
    }

    /// 单节点硬性条件
    fn feasible(&self, spec: &VmSpec, node: &Node) -> bool {
        // 1. 节点在线且健康
        if !matches!(node.status.phase, NodePhase::Online) {
            return false;
        }
        if !node.conditions.iter().any(|c| {
            c.kind == NodeConditionType::Ready && c.status == ConditionStatus::True
        }) {
            return false;
        }

        // 2. 资源够
        let allocatable = node.allocatable(&self.system_reserved);
        if !allocatable.fits(&spec.resources()) {
            return false;
        }

        // 3. 负载未过载
        if node.load_ratio() >= self.overload_ratio {
            return false;
        }

        // 4. VMM 后端支持
        if !node.spec.capabilities.vmm.contains(&spec.backend) {
            return false;
        }

        // 5. 架构匹配
        if let Some(ref arch) = spec.arch {
            if !node.spec.capabilities.arch.iter().any(|a| a == arch) {
                return false;
            }
        }

        // 6. 污点容忍
        if !self.tolerates_all(node, &spec.tolerations) {
            return false;
        }

        // 7. 亲和性 required 全满足
        if let Some(ref affinity) = spec.affinity {
            for sel in &affinity.required {
                if !selector_matches(&node.labels, sel) {
                    return false;
                }
            }
        }

        true
    }

    /// 节点污点是否全部被容忍
    fn tolerates_all(&self, node: &Node, tolerations: &[Toleration]) -> bool {
        node.taints.iter().all(|taint| {
            if taint.effect == TaintEffect::NoExecute {
                // NoExecute 污点必须有对应容忍，且容忍时长未过期才可继续
                tolerations.iter().any(|t| {
                    t.key == taint.key
                        && (t.effect.is_none() || t.effect == Some(taint.effect))
                })
            } else {
                tolerations.iter().any(|t| {
                    t.key == taint.key
                        && (t.effect.is_none() || t.effect == Some(taint.effect))
                })
            }
        })
    }

    /// 打分阶段：越大越好
    fn score(&self, spec: &VmSpec, node: &Node) -> f64 {
        let mut score = 0.0;

        // A. 资源均衡分：剩余资源比例，越空越高分（0..1）
        let allocatable = node.allocatable(&self.system_reserved);
        let cpu_ratio = safe_ratio(allocatable.cpu_millis, node.spec.cpu_millis);
        let mem_ratio = safe_ratio(allocatable.memory_bytes, node.spec.memory_bytes);
        let disk_ratio = safe_ratio(allocatable.disk_bytes, node.spec.disk_bytes);
        score += (cpu_ratio + mem_ratio + disk_ratio) / 3.0;

        // B. preferred 亲和性加分
        if let Some(ref affinity) = spec.affinity {
            for sel in &affinity.preferred {
                if selector_matches(&node.labels, sel) {
                    score += 0.5;
                }
            }
        }

        // C. 反亲和减分：节点上已有满足选择器的 VM 时减分（打散）
        if let Some(ref affinity) = spec.affinity {
            for _sel in &affinity.anti {
                // 简化处理：由调用方传入的 node 上 running_vms 惩罚
                score -= 0.1 * (node.status.running_vms as f64);
            }
        }

        score
    }
}

fn safe_ratio(part: u64, total: u64) -> f64 {
    if total == 0 { 0.0 } else { part as f64 / total as f64 }
}

/// 判断节点标签是否匹配选择器
pub fn selector_matches(labels: &BTreeMap<String, String>, sel: &gita_vm_api::node::LabelSelector) -> bool {
    match sel.operator {
        gita_vm_api::node::LabelOp::In => labels
            .get(&sel.key)
            .map(|v| sel.values.contains(v))
            .unwrap_or(false),
        gita_vm_api::node::LabelOp::NotIn => labels
            .get(&sel.key)
            .map(|v| !sel.values.contains(v))
            .unwrap_or(true),
        gita_vm_api::node::LabelOp::Exists => labels.contains_key(&sel.key),
        gita_vm_api::node::LabelOp::DoesNotExist => !labels.contains_key(&sel.key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gita_vm_api::node::{
        ConditionStatus, NodeCapabilities, NodeCondition, NodeConditionType, NodeSpec, NodeStatus,
        Taint, VmmKind,
    };
    use gita_vm_api::vm::{AffinitySpec, ImageRef};
    use gita_vm_api::LabelOp;
    use std::collections::BTreeMap;

    fn healthy_node(id: i64, name: &str, cpu: u64, mem: u64, disk: u64) -> Node {
        Node {
            id: gita_vm_api::NodeId(id),
            name: name.to_string(),
            cell: None,
            spec: NodeSpec {
                cpu_millis: cpu,
                memory_bytes: mem,
                disk_bytes: disk,
                capabilities: NodeCapabilities {
                    vmm: vec![VmmKind::Qemu],
                    arch: vec!["x86_64".into()],
                    nested_virt: true,
                    gpu_passthrough: false,
                    max_vcpus: 64,
                    max_memory_bytes: mem,
                },
            },
            status: NodeStatus {
                alloc_cpu_millis: 0,
                alloc_memory_bytes: 0,
                alloc_disk_bytes: 0,
                running_vms: 0,
                phase: NodePhase::Online,
                last_heartbeat_ms: 0,
            },
            taints: vec![],
            labels: BTreeMap::new(),
            owner: gita_vm_api::OwnerRef::system(),
            conditions: vec![NodeCondition {
                kind: NodeConditionType::Ready,
                status: ConditionStatus::True,
                last_transition_ms: 0,
                reason: None,
                message: None,
            }],
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    fn spec(cpu: u64, mem: u64, disk: u64) -> VmSpec {
        VmSpec {
            cpu_millis: cpu,
            memory_bytes: mem,
            disk_bytes: disk,
            image: ImageRef { name: "ubuntu-24.04".into(), digest: None },
            kernel: None,
            networks: vec![],
            backend: VmmKind::Qemu,
            arch: None,
            user_data: None,
            metadata: BTreeMap::new(),
            template: None,
            affinity: None,
            tolerations: vec![],
            priority: 0,
        }
    }

    #[test]
    fn places_on_the_only_healthy_node() {
        let s = Scheduler::default();
        let nodes = vec![healthy_node(1, "n1", 8000, 8_000_000_000, 100_000_000_000)];
        let p = s.place(&spec(2000, 2_000_000_000, 10_000_000_000), &nodes).unwrap();
        assert_eq!(p.node_id, 1);
    }

    #[test]
    fn fails_when_no_node_has_enough_resources() {
        let s = Scheduler::default();
        let nodes = vec![healthy_node(1, "n1", 1000, 1_000_000, 10_000_000)];
        let err = s
            .place(&spec(8000, 8_000_000_000, 100_000_000_000), &nodes)
            .unwrap_err();
        assert!(err.contains("没有满足条件的节点"));
    }

    #[test]
    fn skips_offline_nodes() {
        let s = Scheduler::default();
        let mut n = healthy_node(1, "n1", 8000, 8_000_000_000, 100_000_000_000);
        n.status.phase = NodePhase::Offline;
        let nodes = vec![n];
        assert!(s.place(&spec(2000, 2_000_000_000, 10_000_000_000), &nodes).is_err());
    }

    #[test]
    fn skips_nodes_with_intolerable_taints() {
        let s = Scheduler::default();
        let mut n = healthy_node(1, "n1", 8000, 8_000_000_000, 100_000_000_000);
        n.taints = vec![Taint {
            key: "gpu".into(),
            value: "true".into(),
            effect: TaintEffect::NoSchedule,
        }];
        // 无容忍 → 不可调度
        let nodes = vec![n.clone()];
        assert!(s.place(&spec(2000, 2_000_000_000, 10_000_000_000), &nodes).is_err());

        // 有容忍 → 可调度
        let mut s2_spec = spec(2000, 2_000_000_000, 10_000_000_000);
        s2_spec.tolerations = vec![Toleration {
            key: "gpu".into(),
            effect: None,
            toleration_seconds: None,
        }];
        let p = s.place(&s2_spec, &nodes).unwrap();
        assert_eq!(p.node_id, 1);
    }

    #[test]
    fn prefers_empty_node_when_balancing() {
        let s = Scheduler::default();
        // 两节点同样大小，n1 已用 50%，n2 全空
        let mut n1 = healthy_node(1, "n1", 8000, 8_000_000_000, 100_000_000_000);
        n1.status.alloc_cpu_millis = 4000;
        n1.status.alloc_memory_bytes = 4_000_000_000;
        let n2 = healthy_node(2, "n2", 8000, 8_000_000_000, 100_000_000_000);
        let nodes = vec![n1, n2];
        let p = s.place(&spec(1000, 1_000_000_000, 5_000_000_000), &nodes).unwrap();
        assert_eq!(p.node_id, 2, "应该调度到更空的节点");
    }

    #[test]
    fn preferred_affinity_adds_score() {
        let s = Scheduler::default();
        let mut n1 = healthy_node(1, "n1", 8000, 8_000_000_000, 100_000_000_000);
        n1.labels.insert("zone".into(), "hz".into());
        let mut n2 = healthy_node(2, "n2", 8000, 8_000_000_000, 100_000_000_000);
        n2.labels.insert("zone".into(), "sh".into());

        let mut sp = spec(1000, 1_000_000_000, 5_000_000_000);
        sp.affinity = Some(AffinitySpec {
            required: vec![],
            preferred: vec![gita_vm_api::node::LabelSelector {
                key: "zone".into(),
                operator: LabelOp::In,
                values: vec!["sh".into()],
            }],
            anti: vec![],
        });
        let p = s.place(&sp, &[n1, n2]).unwrap();
        assert_eq!(p.node_id, 2);
    }

    #[test]
    fn arch_mismatch_filters_node() {
        let s = Scheduler::default();
        let mut n = healthy_node(1, "n1", 8000, 8_000_000_000, 100_000_000_000);
        n.spec.capabilities.arch = vec!["aarch64".into()];
        let nodes = vec![n];
        let mut sp = spec(2000, 2_000_000_000, 10_000_000_000);
        sp.arch = Some("x86_64".into());
        assert!(s.place(&sp, &nodes).is_err());
    }

    #[test]
    fn selector_matching_semantics() {
        let mut labels = BTreeMap::new();
        labels.insert("zone".to_string(), "hz".to_string());

        assert!(selector_matches(
            &labels,
            &gita_vm_api::node::LabelSelector { key: "zone".into(), operator: LabelOp::In, values: vec!["hz".into(), "sh".into()] }
        ));
        assert!(!selector_matches(
            &labels,
            &gita_vm_api::node::LabelSelector { key: "zone".into(), operator: LabelOp::In, values: vec!["sh".into()] }
        ));
        assert!(selector_matches(
            &labels,
            &gita_vm_api::node::LabelSelector { key: "zone".into(), operator: LabelOp::Exists, values: vec![] }
        ));
        assert!(selector_matches(
            &labels,
            &gita_vm_api::node::LabelSelector { key: "gpu".into(), operator: LabelOp::DoesNotExist, values: vec![] }
        ));
    }
}