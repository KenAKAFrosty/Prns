use std::collections::{BTreeMap, BTreeSet};

use super::{
    CallEdge, KnownCallPath, KnownCallPathFrame, StackAnalysisGapKind, StackFrame,
    StackMetadataError, StackRoot, StackRootRole,
};
use crate::analysis::executable::architecture::{AssuranceAdapter, CallTarget, DecodedInstruction};
use crate::analysis::executable::{
    FunctionAnalysis, FunctionBoundary, StartupAnchorRole, StartupStructure,
};

pub(super) struct CallGraphAnalysis {
    pub(super) roots: Vec<StackRoot>,
    pub(super) edges: Vec<CallEdge>,
    pub(super) largest_path: KnownCallPath,
    pub(super) gaps: BTreeMap<StackAnalysisGapKind, u64>,
}

struct FunctionNode<'a> {
    boundary: &'a FunctionBoundary,
    frame_bytes: u64,
}

#[derive(Clone, Copy)]
enum FunctionLookup {
    SortedDisjoint,
    OverlappingOrUnordered,
}

impl FunctionLookup {
    fn new(nodes: &[FunctionNode<'_>]) -> Self {
        if nodes.windows(2).all(|pair| {
            pair[0].boundary.range.start() <= pair[1].boundary.range.start()
                && pair[0].boundary.range.end() <= pair[1].boundary.range.start()
        }) {
            Self::SortedDisjoint
        } else {
            Self::OverlappingOrUnordered
        }
    }

    fn find(self, nodes: &[FunctionNode<'_>], address: u64) -> Option<usize> {
        match self {
            Self::SortedDisjoint => nodes
                .partition_point(|node| node.boundary.range.start() <= address)
                .checked_sub(1)
                .filter(|index| address < nodes[*index].boundary.range.end()),
            Self::OverlappingOrUnordered => node_containing_linear(nodes, address),
        }
    }
}

pub(super) fn analyze(
    adapter: &AssuranceAdapter,
    startup: &StartupStructure,
    functions: &FunctionAnalysis,
    frames: &[StackFrame],
    instructions: &[DecodedInstruction],
) -> Result<CallGraphAnalysis, StackMetadataError> {
    let nodes = function_nodes(adapter, functions, frames);
    let lookup = FunctionLookup::new(&nodes);
    let mut gaps = BTreeMap::new();
    let mut edges = Vec::new();
    let mut adjacency = vec![Vec::new(); nodes.len()];
    for (index, instruction) in instructions.iter().enumerate() {
        let target = adapter.call_target(instructions, index);
        if target == CallTarget::NotCall {
            continue;
        }
        let Some(source) = lookup.find(&nodes, instruction.address) else {
            increment(&mut gaps, StackAnalysisGapKind::CallSiteOutsideFunction);
            continue;
        };
        let address = match target {
            CallTarget::Direct(address) => address,
            CallTarget::UnresolvedDirect => {
                increment(&mut gaps, StackAnalysisGapKind::UnresolvedDirectCall);
                continue;
            }
            CallTarget::Indirect => {
                increment(&mut gaps, StackAnalysisGapKind::IndirectCall);
                continue;
            }
            CallTarget::NotCall => continue,
        };
        let Some(destination) = lookup.find(&nodes, adapter.normalize_code_address(address)) else {
            increment(&mut gaps, StackAnalysisGapKind::DirectCallOutsideFunctions);
            continue;
        };
        if !adjacency[source].contains(&destination) {
            adjacency[source].push(destination);
        }
        edges.push(CallEdge {
            caller: nodes[source].boundary.name.clone(),
            caller_address: nodes[source].boundary.range.start(),
            callee: nodes[destination].boundary.name.clone(),
            callee_address: nodes[destination].boundary.range.start(),
            call_site: instruction.address,
        });
    }
    edges.sort_by(|left, right| {
        left.call_site
            .cmp(&right.call_site)
            .then_with(|| left.caller.cmp(&right.caller))
            .then_with(|| left.callee.cmp(&right.callee))
    });
    edges.dedup_by(|left, right| {
        left.call_site == right.call_site
            && left.caller_address == right.caller_address
            && left.callee_address == right.callee_address
    });

    let roots = roots(adapter, startup, &nodes, lookup, &mut gaps);
    let root_nodes = roots
        .iter()
        .filter_map(|root| lookup.find(&nodes, root.address))
        .collect::<BTreeSet<_>>();
    let mut states = vec![VisitState::Unvisited; nodes.len()];
    let mut memo = vec![None; nodes.len()];
    let mut largest_path = KnownCallPath {
        bytes: 0,
        frames: Vec::new(),
    };
    for root in root_nodes {
        let candidate = longest_from(root, &nodes, &adjacency, &mut states, &mut memo, &mut gaps)?;
        if candidate.bytes > largest_path.bytes {
            largest_path = candidate;
        }
    }

    Ok(CallGraphAnalysis {
        roots,
        edges,
        largest_path,
        gaps,
    })
}

fn function_nodes<'a>(
    adapter: &AssuranceAdapter,
    functions: &'a FunctionAnalysis,
    frames: &[StackFrame],
) -> Vec<FunctionNode<'a>> {
    let frame_bytes = frames
        .iter()
        .map(|frame| (adapter.normalize_code_address(frame.address), frame.bytes))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    functions
        .boundaries
        .iter()
        .filter(|boundary| {
            seen.insert((
                adapter.normalize_code_address(boundary.range.start()),
                adapter.normalize_code_address(boundary.range.end()),
            ))
        })
        .map(|boundary| FunctionNode {
            boundary,
            frame_bytes: frame_bytes
                .get(&adapter.normalize_code_address(boundary.range.start()))
                .copied()
                .unwrap_or(0),
        })
        .collect()
}

fn roots(
    adapter: &AssuranceAdapter,
    startup: &StartupStructure,
    nodes: &[FunctionNode<'_>],
    lookup: FunctionLookup,
    gaps: &mut BTreeMap<StackAnalysisGapKind, u64>,
) -> Vec<StackRoot> {
    let mut roots = Vec::new();
    for anchor in &startup.anchors {
        let role = match anchor.role {
            StartupAnchorRole::EntryPoint | StartupAnchorRole::ResetVector => {
                StackRootRole::Startup
            }
            StartupAnchorRole::TrapVector => StackRootRole::Trap,
            StartupAnchorRole::InitialStackPointer | StartupAnchorRole::ExceptionVectors => {
                continue
            }
        };
        let address = adapter.normalize_code_address(anchor.address);
        let Some(index) = lookup.find(nodes, address) else {
            increment(gaps, StackAnalysisGapKind::RootOutsideFunctions);
            continue;
        };
        roots.push(StackRoot {
            role,
            name: nodes[index].boundary.name.clone(),
            address: nodes[index].boundary.range.start(),
        });
    }
    for node in nodes
        .iter()
        .filter(|node| node.boundary.is_embassy_task_poll)
    {
        roots.push(StackRoot {
            role: StackRootRole::EmbassyTask,
            name: node.boundary.name.clone(),
            address: node.boundary.range.start(),
        });
    }
    roots.sort_by(|left, right| {
        left.address
            .cmp(&right.address)
            .then_with(|| left.role.cmp(&right.role))
            .then_with(|| left.name.cmp(&right.name))
    });
    roots.dedup();
    roots
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Unvisited,
    Visiting,
    Complete,
}

fn longest_from(
    node: usize,
    nodes: &[FunctionNode<'_>],
    adjacency: &[Vec<usize>],
    states: &mut [VisitState],
    memo: &mut [Option<KnownCallPath>],
    gaps: &mut BTreeMap<StackAnalysisGapKind, u64>,
) -> Result<KnownCallPath, StackMetadataError> {
    if states[node] == VisitState::Visiting {
        increment(gaps, StackAnalysisGapKind::RecursiveCallCycle);
        return Ok(KnownCallPath {
            bytes: 0,
            frames: Vec::new(),
        });
    }
    if let Some(path) = &memo[node] {
        return Ok(path.clone());
    }
    states[node] = VisitState::Visiting;
    let mut child_path = KnownCallPath {
        bytes: 0,
        frames: Vec::new(),
    };
    for child in &adjacency[node] {
        let candidate = longest_from(*child, nodes, adjacency, states, memo, gaps)?;
        if candidate.bytes > child_path.bytes {
            child_path = candidate;
        }
    }
    states[node] = VisitState::Complete;
    let current = &nodes[node];
    let mut frames = Vec::with_capacity(child_path.frames.len() + 1);
    frames.push(KnownCallPathFrame {
        name: current.boundary.name.clone(),
        address: current.boundary.range.start(),
        frame_bytes: current.frame_bytes,
    });
    frames.extend(child_path.frames);
    let path = KnownCallPath {
        bytes: current
            .frame_bytes
            .checked_add(child_path.bytes)
            .ok_or(StackMetadataError::CallPathOverflow)?,
        frames,
    };
    memo[node] = Some(path.clone());
    Ok(path)
}

fn node_containing_linear(nodes: &[FunctionNode<'_>], address: u64) -> Option<usize> {
    nodes.iter().position(|node| {
        node.boundary.range.start() <= address && address < node.boundary.range.end()
    })
}

pub(super) fn increment(
    gaps: &mut BTreeMap<StackAnalysisGapKind, u64>,
    kind: StackAnalysisGapKind,
) {
    *gaps.entry(kind).or_insert(0) += 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_accumulation_keeps_direction_and_root_path() {
        let boundaries = [
            boundary("root", 0x1000, 0x1010),
            boundary("short", 0x1010, 0x1020),
            boundary("deep", 0x1020, 0x1030),
        ];
        let nodes = [
            FunctionNode {
                boundary: &boundaries[0],
                frame_bytes: 8,
            },
            FunctionNode {
                boundary: &boundaries[1],
                frame_bytes: 4,
            },
            FunctionNode {
                boundary: &boundaries[2],
                frame_bytes: 24,
            },
        ];
        let mut states = vec![VisitState::Unvisited; nodes.len()];
        let mut memo = vec![None; nodes.len()];
        let mut gaps = BTreeMap::new();
        let path = longest_from(
            0,
            &nodes,
            &[vec![1, 2], vec![], vec![]],
            &mut states,
            &mut memo,
            &mut gaps,
        )
        .expect("known path");

        assert_eq!(path.bytes, 32);
        assert_eq!(
            path.frames
                .iter()
                .map(|frame| frame.name.as_str())
                .collect::<Vec<_>>(),
            ["root", "deep"]
        );
    }

    #[test]
    fn call_path_byte_overflow_is_rejected() {
        let boundaries = [
            boundary("root", 0x1000, 0x1010),
            boundary("child", 0x1010, 0x1020),
        ];
        let nodes = [
            FunctionNode {
                boundary: &boundaries[0],
                frame_bytes: u64::MAX,
            },
            FunctionNode {
                boundary: &boundaries[1],
                frame_bytes: 1,
            },
        ];
        let mut states = vec![VisitState::Unvisited; nodes.len()];
        let mut memo = vec![None; nodes.len()];

        assert_eq!(
            longest_from(
                0,
                &nodes,
                &[vec![1], vec![]],
                &mut states,
                &mut memo,
                &mut BTreeMap::new(),
            ),
            Err(StackMetadataError::CallPathOverflow)
        );
    }

    #[test]
    fn sorted_disjoint_functions_use_exact_boundary_lookup() {
        let boundaries = [
            boundary("first", 0x1000, 0x1010),
            boundary("second", 0x1010, 0x1020),
        ];
        let nodes = boundaries
            .iter()
            .map(|boundary| FunctionNode {
                boundary,
                frame_bytes: 0,
            })
            .collect::<Vec<_>>();
        let lookup = FunctionLookup::new(&nodes);

        assert_eq!(lookup.find(&nodes, 0x0fff), None);
        assert_eq!(lookup.find(&nodes, 0x1000), Some(0));
        assert_eq!(lookup.find(&nodes, 0x100f), Some(0));
        assert_eq!(lookup.find(&nodes, 0x1010), Some(1));
        assert_eq!(lookup.find(&nodes, 0x1020), None);
    }

    #[test]
    fn overlapping_functions_preserve_first_match_semantics() {
        let boundaries = [
            boundary("outer", 0x1000, 0x1030),
            boundary("inner", 0x1010, 0x1020),
        ];
        let nodes = boundaries
            .iter()
            .map(|boundary| FunctionNode {
                boundary,
                frame_bytes: 0,
            })
            .collect::<Vec<_>>();
        let lookup = FunctionLookup::new(&nodes);

        assert!(matches!(lookup, FunctionLookup::OverlappingOrUnordered));
        assert_eq!(lookup.find(&nodes, 0x1015), Some(0));
    }

    fn boundary(name: &str, start: u64, end: u64) -> FunctionBoundary {
        FunctionBoundary {
            name: name.to_string(),
            range: personal_hopspot_memory::AddressRange::new(start, end),
            fingerprint: String::new(),
            is_embassy_task_poll: false,
        }
    }
}
