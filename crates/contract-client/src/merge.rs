//! Pure helpers that combine PortalRegistry and legacy GatewayRegistry/AllocationsViewer reads
//! into the trait-level shapes the rest of the network expects.
//!
//! Keeping the merge logic out of the async trait methods makes it unit-testable without RPC
//! scaffolding. The async methods stay responsible for fetching; these functions only reshape.

use std::collections::{BTreeMap, HashSet};

use ethers::types::{Address, U256};
use libp2p::PeerId;

use crate::contracts;
use crate::PortalCluster;

/// Concatenate active portals from the new PortalRegistry and the legacy GatewayRegistry,
/// preserving order within each source and de-duping by `PeerId`. New-source occurrences win
/// position when the same peer ID appears in both (i.e. a migrated portal won't be double-listed
/// and won't move to the legacy slot).
pub(crate) fn merge_active_portals(
    new_portals: Vec<PeerId>,
    legacy_portals: Vec<PeerId>,
) -> Vec<PeerId> {
    let mut seen: HashSet<PeerId> = HashSet::new();
    new_portals
        .into_iter()
        .chain(legacy_portals)
        .filter(|p| seen.insert(*p))
        .collect()
}

/// Merge new-registry clusters with legacy AllocationsViewer rows into the `Vec<PortalCluster>`
/// shape downstream workers consume.
///
/// Invariant: **one `PortalCluster` per operator** in the output.
///
/// Steps:
/// 1. Build the migrated peer-ID set from `new_clusters` (the union of every cluster's
///    `portal_ids`).
/// 2. Drop legacy allocations whose `gateway_id` is in that set — those portals are already
///    served by the new registry, so re-counting them would double-allocate.
/// 3. Insert new clusters into a `BTreeMap<Address, _>` keyed by operator. On collision:
///    extend `portal_ids` (deduped) and add `allocated_computation_units` saturatingly.
/// 4. Insert surviving legacy rows into the same map, preserving the old client behavior:
///    append all portal IDs, but add only the first legacy `allocated` value per operator.
/// 5. Return the map's values. `BTreeMap` iteration is by operator address ordering, giving
///    callers a deterministic `Vec` for the same input.
pub(crate) fn merge_portal_clusters(
    new_clusters: Vec<PortalCluster>,
    legacy_allocations: Vec<contracts::Allocation>,
) -> Vec<PortalCluster> {
    let migrated_peers: HashSet<PeerId> =
        new_clusters.iter().flat_map(|c| c.portal_ids.iter().copied()).collect();

    let mut by_operator: BTreeMap<Address, PortalCluster> = BTreeMap::new();
    let mut legacy_cu_seen: HashSet<Address> = HashSet::new();

    for cluster in new_clusters {
        upsert_cluster(
            &mut by_operator,
            cluster.operator_addr,
            cluster.portal_ids,
            cluster.allocated_computation_units,
        );
    }

    for allocation in legacy_allocations {
        let Ok(peer_id) = PeerId::from_bytes(&allocation.gateway_id) else {
            continue;
        };
        if migrated_peers.contains(&peer_id) {
            continue;
        }
        upsert_legacy_allocation(
            &mut by_operator,
            &mut legacy_cu_seen,
            allocation.operator,
            peer_id,
            allocation.allocated,
        );
    }

    by_operator.into_values().collect()
}

/// Insert an additive cluster contribution into the operator-keyed map, deduping peer IDs and
/// saturating CU sums so two `U256::MAX` rows never panic.
fn upsert_cluster(
    map: &mut BTreeMap<Address, PortalCluster>,
    operator: Address,
    portal_ids: Vec<PeerId>,
    cus: U256,
) {
    map.entry(operator)
        .and_modify(|existing| {
            for pid in &portal_ids {
                if !existing.portal_ids.contains(pid) {
                    existing.portal_ids.push(*pid);
                }
            }
            existing.allocated_computation_units =
                existing.allocated_computation_units.saturating_add(cus);
        })
        .or_insert_with(|| PortalCluster {
            operator_addr: operator,
            portal_ids,
            allocated_computation_units: cus,
        });
}

/// Insert a legacy allocation while preserving the old first-CU-wins behavior per operator.
fn upsert_legacy_allocation(
    map: &mut BTreeMap<Address, PortalCluster>,
    legacy_cu_seen: &mut HashSet<Address>,
    operator: Address,
    peer_id: PeerId,
    cus: U256,
) {
    let contribution = if legacy_cu_seen.insert(operator) {
        cus
    } else {
        U256::zero()
    };
    upsert_cluster(map, operator, vec![peer_id], contribution);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(seed: u8) -> PeerId {
        // Deterministic PeerIds from a 32-byte seed via libp2p Ed25519 keypair derivation.
        let bytes = [seed; 32];
        let kp = libp2p::identity::Keypair::ed25519_from_bytes(bytes).unwrap();
        kp.public().to_peer_id()
    }

    fn addr(seed: u8) -> Address {
        let mut bytes = [0u8; 20];
        bytes[19] = seed;
        Address::from(bytes)
    }

    fn allocation(peer: PeerId, op: Address, cus: u64) -> contracts::Allocation {
        contracts::Allocation {
            gateway_id: ethers::types::Bytes::from(peer.to_bytes()),
            allocated: U256::from(cus),
            operator: op,
        }
    }

    fn cluster(op: Address, peers: Vec<PeerId>, cus: u64) -> PortalCluster {
        PortalCluster {
            operator_addr: op,
            portal_ids: peers,
            allocated_computation_units: U256::from(cus),
        }
    }

    // ---- merge_active_portals ----

    #[test]
    fn active_portals_both_empty_returns_empty() {
        assert!(merge_active_portals(vec![], vec![]).is_empty());
    }

    #[test]
    fn active_portals_legacy_only_passthrough() {
        let legacy = vec![pid(1), pid(2), pid(3)];
        assert_eq!(merge_active_portals(vec![], legacy.clone()), legacy);
    }

    #[test]
    fn active_portals_new_only_passthrough() {
        let new = vec![pid(1), pid(2)];
        assert_eq!(merge_active_portals(new.clone(), vec![]), new);
    }

    #[test]
    fn active_portals_overlap_dedupes_new_wins_position() {
        // pid(2) is in both; it should appear once at the new-source position (index 1),
        // not at the legacy position (index 3).
        let new = vec![pid(1), pid(2), pid(3)];
        let legacy = vec![pid(4), pid(5), pid(2), pid(6)];
        let out = merge_active_portals(new, legacy);
        assert_eq!(out, vec![pid(1), pid(2), pid(3), pid(4), pid(5), pid(6)]);
    }

    #[test]
    fn active_portals_dedupes_within_new() {
        let new = vec![pid(1), pid(2), pid(1), pid(3)];
        let out = merge_active_portals(new, vec![]);
        assert_eq!(out, vec![pid(1), pid(2), pid(3)]);
    }

    // ---- merge_portal_clusters ----

    #[test]
    fn clusters_both_empty_returns_empty() {
        assert!(merge_portal_clusters(vec![], vec![]).is_empty());
    }

    #[test]
    fn clusters_legacy_only_equal_strategy_one_operator_n_portals() {
        // Legacy behavior: one operator can own several gateways backed by the same stake.
        // Keep the first CU value and append all peer IDs.
        let op = addr(1);
        let legacy = vec![allocation(pid(1), op, 100), allocation(pid(2), op, 100)];
        let out = merge_portal_clusters(vec![], legacy);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].operator_addr, op);
        assert_eq!(out[0].portal_ids.len(), 2);
        assert!(out[0].portal_ids.contains(&pid(1)));
        assert!(out[0].portal_ids.contains(&pid(2)));
        assert_eq!(out[0].allocated_computation_units, U256::from(100));
    }

    #[test]
    fn clusters_legacy_only_different_allocations_keeps_first() {
        let op = addr(1);
        let legacy = vec![allocation(pid(1), op, 70), allocation(pid(2), op, 30)];
        let out = merge_portal_clusters(vec![], legacy);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].allocated_computation_units, U256::from(70));
    }

    #[test]
    fn clusters_legacy_row_for_migrated_peer_is_dropped() {
        let op = addr(1);
        let new = vec![cluster(op, vec![pid(1)], 500)];
        let legacy = vec![allocation(pid(1), op, 999)];
        let out = merge_portal_clusters(new, legacy);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].portal_ids, vec![pid(1)]);
        // Legacy 999 dropped because pid(1) is migrated; only the new cluster's 500 remains.
        assert_eq!(out[0].allocated_computation_units, U256::from(500));
    }

    #[test]
    fn clusters_same_operator_in_both_sources_merges() {
        let op = addr(1);
        let new = vec![cluster(op, vec![pid(1)], 100)];
        let legacy = vec![allocation(pid(2), op, 200)]; // not migrated
        let out = merge_portal_clusters(new, legacy);
        assert_eq!(out.len(), 1, "one PortalCluster per operator invariant");
        assert!(out[0].portal_ids.contains(&pid(1)));
        assert!(out[0].portal_ids.contains(&pid(2)));
        assert_eq!(out[0].allocated_computation_units, U256::from(300));
    }

    #[test]
    fn clusters_same_operator_adds_only_one_legacy_cu_to_new() {
        let op = addr(1);
        let new = vec![cluster(op, vec![pid(1)], 100)];
        let legacy = vec![allocation(pid(2), op, 200), allocation(pid(3), op, 200)];
        let out = merge_portal_clusters(new, legacy);
        assert_eq!(out.len(), 1);
        assert!(out[0].portal_ids.contains(&pid(1)));
        assert!(out[0].portal_ids.contains(&pid(2)));
        assert!(out[0].portal_ids.contains(&pid(3)));
        assert_eq!(out[0].allocated_computation_units, U256::from(300));
    }

    #[test]
    fn clusters_two_new_clusters_same_operator_collapse() {
        let op = addr(1);
        let new = vec![cluster(op, vec![pid(1)], 100), cluster(op, vec![pid(2)], 200)];
        let out = merge_portal_clusters(new, vec![]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].portal_ids.len(), 2);
        assert_eq!(out[0].allocated_computation_units, U256::from(300));
    }

    #[test]
    fn clusters_portal_registry_cu_passes_through() {
        let op = addr(1);
        let new = vec![cluster(op, vec![pid(1)], 42)];
        let out = merge_portal_clusters(new, vec![]);
        assert_eq!(out[0].allocated_computation_units, U256::from(42));
    }

    #[test]
    fn clusters_stable_iteration_order_by_operator() {
        // Insert two operators in reverse-order; output must be sorted by Address (BTreeMap).
        let op_a = addr(1);
        let op_b = addr(2);
        let new = vec![cluster(op_b, vec![pid(2)], 20), cluster(op_a, vec![pid(1)], 10)];
        let out_first = merge_portal_clusters(new.clone(), vec![]);
        let out_second = merge_portal_clusters(new, vec![]);
        assert_eq!(out_first.len(), 2);
        assert_eq!(out_first[0].operator_addr, op_a);
        assert_eq!(out_first[1].operator_addr, op_b);
        // PortalCluster has no PartialEq (stable public surface); compare via Debug.
        assert_eq!(format!("{out_first:?}"), format!("{out_second:?}"), "deterministic ordering");
    }

    #[test]
    fn clusters_two_new_cluster_cus_are_summed() {
        let op = addr(1);
        let new = vec![cluster(op, vec![pid(1)], u64::MAX), cluster(op, vec![pid(2)], u64::MAX)];
        let mut out = merge_portal_clusters(new, vec![]);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out.pop().unwrap().allocated_computation_units,
            U256::from(u64::MAX).saturating_add(U256::from(u64::MAX))
        );
    }

    #[test]
    fn clusters_new_cu_sum_saturates_at_u256_max() {
        let op = addr(1);
        let new = vec![
            PortalCluster {
                operator_addr: op,
                portal_ids: vec![pid(1)],
                allocated_computation_units: U256::MAX,
            },
            cluster(op, vec![pid(2)], 1),
        ];
        let out = merge_portal_clusters(new, vec![]);
        assert_eq!(out[0].allocated_computation_units, U256::MAX);
    }
}
