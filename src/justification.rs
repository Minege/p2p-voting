use std::collections::{BTreeSet, HashSet, HashMap};
use std::collections::btree_set::{Iter};
use std::fmt::{Debug, Formatter, Result};
// use std::io::{Error};
use message::{AbstractMsg, Message};
use rayon::collections::btree_set::Iter as ParIter;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use weight_unit::{WeightUnit};
use traits::{Zero, Sender, Estimate, Data};
use senders_weight::SendersWeight;

#[derive(Eq, Ord, PartialOrd, PartialEq, Clone, Default, Hash)]
pub struct Justification<M: AbstractMsg>(BTreeSet<M>);

impl<M: AbstractMsg> Justification<M> {
    // Re-exports from BTreeSet wrapping M
    pub fn new() -> Self {
        Justification(BTreeSet::new())
    }
    pub fn iter(&self) -> Iter<M> {
        self.0.iter()
    }
    pub fn par_iter(&self) -> ParIter<M> {
        self.0.par_iter()
    }
    pub fn contains(&self, msg: &M) -> bool {
        self.0.contains(msg)
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn insert(&mut self, msg: M) -> bool {
        self.0.insert(msg)
    }
    fn head(&self) -> Option<&M> {
        self.0.iter().next()
    }

    // reexport from Estimate impl
    pub fn mk_estimate(
        &self,
        finalized_msg: Option<&M>,
        weights: &Weights<<M as AbstractMsg>::Sender>,
        data: Option<<M as Data>::Data>,
    ) -> M::Estimate {
        M::Estimate::mk_estimate(self, finalized_msg, weights, data)
    }

    // Custom functions
    /// get the additional equivocators upon insertion of msg to the state. note
    /// that if equivocator is a recurrent equivocator, it will be found again
    /// here. i believe the weight of an equivocator has to be set to ZERO
    /// immediately upon finding an equivocation
    fn get_equivocators(&self, msg_new: &M) -> HashSet<M::Sender> {
        self.par_iter()
            .filter_map(|msg_old| {
                if msg_old.equivocates(&msg_new) {
                    let equivocator = msg_old.get_sender();
                    Some(equivocator.clone())
                }
                else {
                    None
                }
            })
            .collect()
    }

    /// insert msgs to the Justification, accepting up to $thr$ faults by
    /// weight, returns success=true if at least one msg of the set gets
    /// successfully included to the justification
    pub fn faulty_inserts(
        &mut self,
        msgs: HashSet<&M>,
        weights: &Weights<M::Sender>,
    ) -> FaultyInsertResult<M::Sender> {
        /// get msgs and fault weight overhead and equivocators overhead sorted
        /// by fault weight overhead
        fn get_msgs_sorted_by_faultweight<'z, M: AbstractMsg>(
            justification: &Justification<M>,
            Weights {
                senders_weights,
                equivocators,
                ..
            }: &Weights<M::Sender>,
            msgs: HashSet<&'z M>,
        ) -> Vec<&'z M> {
            let mut msgs_sorted_by_faultw: Vec<_> = msgs
                .iter()
                .enumerate()
                .filter_map(|(outer_i, &msg)| {
                    // equivocations in relation to state
                    let state_equvctrs: HashSet<_> =
                        justification.get_equivocators(msg);

                    // equivocations present within the current latest_msgs set that
                    // we're trying to insert to the state
                    let pairwise_equivocators: HashSet<_> = msgs
                        .iter()
                    // ensures we check only once per pair [(a, b)] and not
                    // [(a, b), (b, a)]
                        .skip(outer_i)
                        .filter_map(|m| {
                            if m.equivocates(msg) { Some(m.get_sender().clone()) }
                            else { None }
                        })
                        .collect();

                    let msg_equivocators: HashSet<_> = state_equvctrs
                        .union(&pairwise_equivocators)
                    // take only the equivocators that are not yet on the
                    // equivocator set as the others already have their
                    // weight counted into the state fault count
                        .filter(|equivocator| !equivocators.contains(equivocator))
                        .cloned()
                        .collect();
                    let msg_faultweight_overhead =
                        senders_weights.sum_weight_senders(&msg_equivocators);
                    // sum_weight_senders returns nan if a sender is not found
                    if msg_faultweight_overhead.is_nan() {
                        None
                    }
                    else {
                        Some((msg, msg_faultweight_overhead))
                    }
                })
                .collect();
            let _ = msgs_sorted_by_faultw.sort_unstable_by(
                |(_, w0), (_, w1)| {
                    w0.partial_cmp(w1).unwrap_or(::std::cmp::Ordering::Greater) // tie breaker
                },
            );

            // return a Vec<AbstractMsg>
            msgs_sorted_by_faultw
                .iter()
                .map(|(m, _)| m)
                .cloned()
                .collect()
        }

        // do the actual insertions to the state
        get_msgs_sorted_by_faultweight(&self, &weights, msgs)
            .iter()
            .fold(
                FaultyInsertResult {
                    success: false,
                    weights: weights.clone(),
                },
                |acc, &msg| {
                    let FaultyInsertResult { success, weights } =
                        self.faulty_insert(msg, &acc.weights);
                    FaultyInsertResult {
                        success: acc.success || success,
                        weights,
                    }
                },
            )
    }

    pub fn faulty_insert(
        &mut self,
        msg: &M,
        weights: &Weights<M::Sender>,
    ) -> FaultyInsertResult<M::Sender> {
        let weights = weights.clone();
        let msg_equivocators_overhead: HashSet<_> = self.get_equivocators(msg)
            .iter()
        // take only the msg_eoquivocators_overhead that are not yet on the
        // equivocator set as the others already have their weight counted into
        // the state fault count
            .filter(|equivocator| !weights.equivocators.contains(equivocator))
            .cloned()
            .collect();
        let msg_fault_weight_overhead = weights
            .senders_weights
            .sum_weight_senders(&msg_equivocators_overhead);
        let new_fault_weight =
            weights.state_fault_weight + msg_fault_weight_overhead;
        let equivocators = weights
            .equivocators
            .union(&msg_equivocators_overhead)
            .cloned()
            .collect();

        if new_fault_weight <= weights.thr {
            let success = self.insert(msg.clone());
            let weights = if success {
                Weights {
                    state_fault_weight: new_fault_weight,
                    equivocators,
                    ..weights
                }
            }
            else {
                Weights { ..weights }
            };

            FaultyInsertResult { success, weights }
        }
        // conflicting message NOT added to the set as it crosses the fault
        // weight thr OR msg_fault_weight_overhead is NAN (from the unwrap)
        else {
            FaultyInsertResult {
                success: false,
                weights: Weights {
                    // equivocators,
                    ..weights
                },
            }
        }
    }

    /// The Greedy Heaviest-Observed Sub-Tree
    pub fn ghost(
        &self,
        finalized_msg: Option<&M>,
        senders_weights: &SendersWeight<M::Sender>,
    ) -> Option<M> {
        self.get_subtree_weights(finalized_msg, senders_weights)
            .iter()
            .max_by(|(_, &weight0), (_, &weight1)| {
                weight0
                    .partial_cmp(&weight1)
                // tie breaker, the unwrap fails because both values are the
                // same and we arbitrarily chose a result. TODO this should be
                // something deterministic, like blockhash
                    .unwrap_or(::std::cmp::Ordering::Greater)
            })
            .map(|(heaviest_subtree, _)| heaviest_subtree.clone())
    }

    pub fn get_subtree_weights(
        &self,
        finalized_msg: Option<&M>, // stops sum at finalized_msg
        senders_weights: &SendersWeight<M::Sender>,
    ) -> HashMap<M, WeightUnit> {
        fn recursor<M>(
            m: &M,
            finalized_msg: Option<&M>, // if None, runs all the way to genesis msgs
            senders_weights: &SendersWeight<M::Sender>,
            all_senders: &HashSet<M::Sender>,
            mut senders_referred: HashSet<M::Sender>,
            initial_weight: WeightUnit,
        ) -> WeightUnit
        where
            M: AbstractMsg,
        {
            m.get_justification().iter().fold(
                initial_weight,
                |weight_referred, m_prime| {
                    // base case
                    if finalized_msg
                        .map(|f_msg| {
                            f_msg == m_prime || !m_prime.depends(f_msg) // TODO: check if needed
                        })
                        .unwrap_or(false)
                    {
                        weight_referred
                    }
                    else {
                        let sender_current = m_prime.get_sender();
                        // it fails to insert sender_current, if sender_current is
                        // already in set
                        let weight_referred = if senders_referred
                            .insert(sender_current.clone())
                        {
                            weight_referred
                                + senders_weights
                                    .get_weight(&sender_current)
                                    .unwrap_or(WeightUnit::ZERO)
                        }
                        else {
                            weight_referred
                        };

                        recursor(
                            m_prime,
                            finalized_msg,
                            senders_weights,
                            all_senders,
                            senders_referred.clone(),
                            weight_referred,
                        )
                    }
                },
            )
        };
        // initial state, trigger recursion
        let all_senders = &senders_weights.get_senders();
        self.iter()
            .map(|m| {
                let sender_current = m.get_sender();
                let senders_referred: HashSet<M::Sender> =
                    [sender_current.clone()].iter().cloned().collect();
                let initial_weight = senders_weights
                    .get_weight(&sender_current)
                    .unwrap_or(WeightUnit::ZERO);
                (
                    m.clone(),
                    recursor(
                        m,
                        finalized_msg,
                        senders_weights,
                        all_senders,
                        senders_referred.clone(),
                        initial_weight,
                    ),
                )
            })
            .collect()
    }
}

impl<M: AbstractMsg> Debug for Justification<M> {
    fn fmt(&self, f: &mut Formatter) -> Result {
        write!(f, "{:?}", self.0)
    }
}

// FIXME: success should probably be out of this struct, as this struct can be
// used to keep track of state cummulatively and success is related to a single
// insertion
pub struct FaultyInsertResult<S: Sender> {
    pub success: bool,
    pub weights: Weights<S>,
    // pub equivocators: HashSet<S>,
}

// FIXME: this became more than Weights, should find a better name, or break up in parts
#[derive(Debug, Clone)]
pub struct Weights<S: Sender> {
    senders_weights: SendersWeight<S>,
    state_fault_weight: WeightUnit,
    thr: WeightUnit,
    equivocators: HashSet<S>, // FIXME: put it here as its needed on the same context as
}

impl<S: Sender> Weights<S> {
    pub fn new(
        senders_weights: SendersWeight<S>,
        state_fault_weight: WeightUnit,
        thr: WeightUnit,
        equivocators: HashSet<S>,
    ) -> (Self) {
        Weights {
            senders_weights,
            equivocators,
            state_fault_weight,
            thr,
        }
    }
    pub fn get_senders_weights(&self) -> &SendersWeight<S> {
        &self.senders_weights
    }
}

#[cfg(test)]
mod justification {
    use vote_count::{VoteCount};
    use super::*;

    #[test]
    fn children_weight() {
        use blockchain::{Block, Blockchain};
        let (sender0, sender1, sender2, sender3) = (0, 1, 2, 3); // miner identities
        let (weight0, weight1, weight2, weight3) = (2., 4., 8., 16.); // and their corresponding weights
        let senders_weights = SendersWeight::new(
            [
                (sender0, weight0),
                (sender1, weight1),
                (sender2, weight2),
                (sender3, weight3),
            ].iter()
                .cloned()
                .collect(),
        );
        let estimate = Blockchain::new(None, None);
        let justification = Justification::new();
        let genesis_block =
            Block::new(sender0, justification, estimate.clone());
        assert_eq!(
            genesis_block.get_estimate(),
            &estimate,
            "genesis block with None as prev_block"
        );
        // genesis(s=0, w=2) <- b1(s=1, w=4) <- b2(s=2, w=8) <- b3(s=3, w=16)
        // weights:   2               6               14               30
        let mut justification = Justification::new();
        assert!(justification.insert(genesis_block.clone()));
        let subtree_weights =
            justification.get_subtree_weights(None, &senders_weights);
        let (_msg, weight) = subtree_weights.iter().next().unwrap();
        assert_eq!(weight, &2.0);
        println!("children_weights1: {:?}", subtree_weights);
        let estimate = Blockchain::new(Some(genesis_block.clone()), None);
        let b1 = Block::new(sender1, justification, estimate);

        let mut justification = Justification::new();
        assert!(justification.insert(b1.clone()));
        let subtree_weights =
            justification.get_subtree_weights(None, &senders_weights);
        let (_msg, weight) = subtree_weights.iter().next().unwrap();
        assert_eq!(weight, &6.0);
        let estimate = Blockchain::new(Some(b1), None);
        let b2 = Block::new(sender2, justification, estimate);

        let mut justification = Justification::new();
        assert!(justification.insert(b2.clone()));
        let subtree_weights =
            justification.get_subtree_weights(None, &senders_weights);
        let (_msg, weight) = subtree_weights.iter().next().unwrap();
        assert_eq!(weight, &14.0);
        let estimate = Blockchain::new(Some(b2), None);
        let b3 = Block::new(sender3, justification, estimate);

        let mut justification = Justification::new();
        assert!(justification.insert(b3.clone()));
        let subtree_weights =
            justification.get_subtree_weights(None, &senders_weights);
        let (_msg, weight) = subtree_weights.iter().next().unwrap();
        assert_eq!(weight, &30.0);
    }

    #[test]
    fn faulty_inserts_sorted() {
        let senders_weights = SendersWeight::new(
            [(0, 1.0), (1, 2.0), (2, 3.0)].iter().cloned().collect(),
        );

        let v0 = &VoteCount::create_vote_msg(0, false);
        let v0_prime = &VoteCount::create_vote_msg(0, true);
        let v1 = &VoteCount::create_vote_msg(1, true);
        let v1_prime = &VoteCount::create_vote_msg(1, false);
        let v2 = &VoteCount::create_vote_msg(2, true);
        let v2_prime = &VoteCount::create_vote_msg(2, false); // equivocating vote
        let weights = Weights {
            senders_weights: senders_weights.clone(),
            state_fault_weight: 0.0,
            thr: 3.0,
            equivocators: HashSet::new(),
        };
        let (m0, weights) = &Message::from_msgs(
            0,
            vec![v2, v2_prime, v1, v1_prime, v0, v0_prime],
            None,
            &weights,
            None as Option<VoteCount>,
        );
        assert_eq!(m0.get_justification().len(), 5);
        assert_eq!(weights.state_fault_weight, 3.0);
    }
    #[test]
    fn faulty_inserts() {
        let senders_weights = SendersWeight::new(
            [(0, 1.0), (1, 1.0), (2, 1.0)].iter().cloned().collect(),
        );
        let v0 = &VoteCount::create_vote_msg(0, false);
        let v0_prime = &VoteCount::create_vote_msg(0, true); // equivocating vote
        let v1 = &VoteCount::create_vote_msg(1, true);
        let mut j0 = Justification::new();
        let weights = Weights {
            senders_weights: senders_weights.clone(),
            state_fault_weight: 0.0,
            thr: 0.0,
            equivocators: HashSet::new(),
        };
        assert!(
            j0.faulty_inserts([v0].iter().cloned().collect(), &weights)
                .success
        );
        let (m0, _weights) = &Message::from_msgs(
            0,
            vec![v0],
            None,
            &weights,
            None as Option<VoteCount>,
        );
        // let m0 = &Message::new(0, justification, estimate);
        let mut j1 = Justification::new();
        assert!(
            j1.faulty_inserts(
                vec![v1].iter().cloned().collect(),
                &Weights {
                    senders_weights: senders_weights.clone(),
                    state_fault_weight: 0.0,
                    thr: 0.0,
                    equivocators: HashSet::new(),
                }
            ).success
        );
        assert!(
            j1.faulty_inserts(
                vec![m0].iter().cloned().collect(),
                &Weights {
                    senders_weights: senders_weights.clone(),
                    state_fault_weight: 0.0,
                    thr: 0.0,
                    equivocators: HashSet::new(),
                }
            ).success
        );
        assert!(
            !j1.faulty_inserts(
                vec![v0_prime].iter().cloned().collect(),
                &Weights {
                    senders_weights: senders_weights.clone(),
                    state_fault_weight: 0.0,
                    thr: 0.0,
                    equivocators: HashSet::new(),
                }
            ).success,
            "$v0_prime$ should conflict with $v0$ through $m0$, and we should reject as our fault tolerance thr is zero"
        );
        assert!(
            j1.clone()
                .faulty_inserts(
                    vec![v0_prime].iter().cloned().collect(),
                    &Weights {
                        senders_weights: senders_weights.clone(),
                        state_fault_weight: 0.0,
                        thr: 1.0,
                        equivocators: HashSet::new(),
                    }
                )
                .success,
            "$v0_prime$ conflicts with $v0$ through $m0$, but we should accept this fault as it doesnt cross the fault threshold for the set"
        );

        assert_eq!(
            j1.clone()
                .faulty_inserts(
                    vec![v0_prime].iter().cloned().collect(),
                    &Weights {
                        senders_weights: senders_weights.clone(),
                        state_fault_weight: 0.0,
                        thr: 1.0,
                        equivocators: HashSet::new(),
                    }
                )
                .weights
                .state_fault_weight,
            1.0,
            "$v0_prime$ conflicts with $v0$ through $m0$, but we should accept
this fault as it doesnt cross the fault threshold for the set, and thus the
state_fault_weight should be incremented to 1.0"
        );

        assert!(
            !j1.clone()
                .faulty_inserts(
                    vec![v0_prime].iter().cloned().collect(),
                    &Weights {
                        senders_weights: senders_weights.clone(),
                        state_fault_weight: 0.1,
                        thr: 1.0,
                        equivocators: HashSet::new(),
                    }
                )
                .success,
            "$v0_prime$ conflicts with $v0$ through $m0$, and we should not accept this fault as the fault threshold gets crossed for the set"
        );

        assert_eq!(
            j1.clone()
                .faulty_inserts(
                    vec![v0_prime].iter().cloned().collect(),
                    &Weights {
                        senders_weights: senders_weights.clone(),
                        state_fault_weight: 0.1,
                        thr: 1.0,
                        equivocators: HashSet::new(),
                    }
                )
                .weights.state_fault_weight,
            0.1,
            "$v0_prime$ conflicts with $v0$ through $m0$, and we should NOT accept this fault as the fault threshold gets crossed for the set, and thus the state_fault_weight should not be incremented"
        );

        assert!(
            j1.clone()
                .faulty_inserts(
                    vec![v0_prime].iter().cloned().collect(),
                    &Weights {
                        senders_weights: senders_weights.clone(),
                        state_fault_weight: 1.0,
                        thr: 2.0,
                        equivocators: HashSet::new(),
                    }
                )
                .success,
            "$v0_prime$ conflict with $v0$ through $m0$, but we should accept this fault as the thr doesnt get crossed for the set"
        );

        let senders_weights = SendersWeight::new([].iter().cloned().collect());
        // bug found
        assert!(
            !j1.clone()
                .faulty_inserts(
                    vec![v0_prime].iter().cloned().collect(),
                    &Weights {
                        senders_weights: senders_weights.clone(),
                        state_fault_weight: 1.0,
                        thr: 2.0,
                        equivocators: HashSet::new(),
                    }
                )
                .success,
            "$v0_prime$ conflict with $v0$ through $m0$, but we should NOT accept this fault as we can't know the weight of the sender, which could be Infinity"
        );

        assert_eq!(
            j1.clone()
                .faulty_inserts(
                    vec![v0_prime].iter().cloned().collect(),
                    &Weights {
                        senders_weights: senders_weights.clone(),
                        state_fault_weight: 1.0,
                        thr: 2.0,
                        equivocators: HashSet::new(),
                    }
                )
                .weights.state_fault_weight,
            1.0,
            "$v0_prime$ conflict with $v0$ through $m0$, but we should NOT accept this fault as we can't know the weight of the sender, which could be Infinity, and thus the state_fault_weight should be unchanged"
        );
    }
}
