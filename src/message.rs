use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::fmt::{Debug, Formatter, Result};
use std::sync::{Arc};
// use std::io::{Error};

use rayon::prelude::*;
use traits::{Estimate, Zero, Sender, Data};
use justification::{Justification, Weights, FaultyInsertResult};
use weight_unit::{WeightUnit};
use senders_weight::SendersWeight;

pub trait AbstractMsg:
    Hash + Ord + Clone + Eq + Sync + Send + Debug + Data
{
    // To be implemented on concrete struct
    // type Data: Data;
    type Sender: Sender;
    type Estimate: Estimate<M = Self>;
    fn get_sender(&self) -> &Self::Sender;
    fn get_estimate(&self) -> &Self::Estimate;
    fn get_justification<'z>(&'z self) -> &'z Justification<Self>;
    fn new(
        sender: Self::Sender,
        justification: Justification<Self>,
        estimate: Self::Estimate,
    ) -> Self;

    // Following methods are actual implementations

    /// create a new msg from latest_messages
    fn from_msgs(
        sender: Self::Sender,
        latest_msgs: Vec<&Self>,
        finalized_msg: Option<&Self>,
        weights: &Weights<Self::Sender>,
        external_data: Option<Self::Data>,
    ) -> (Self, Weights<Self::Sender>) {
        // // TODO eventually comment out these lines, and FIXME tests
        // // check whether two messages from same sender
        // let mut senders = HashSet::new();
        // let dup_senders = latest_msgs.iter().any(|msg| !senders.insert(msg.get_sender()));
        // assert!(!dup_senders, "A sender can only have one, and only one, latest message");

        // dedup by putting msgs into a hashset
        let latest_msgs: HashSet<_> = latest_msgs.iter().cloned().collect();

        let mut justification = Justification::new();
        let FaultyInsertResult { success, weights } =
            justification.faulty_inserts(latest_msgs, &weights);
        assert!(success, "None of the messages could be added to the state!");
        let estimate =
            justification.mk_estimate(finalized_msg, &weights, external_data);
        let message = Self::new(sender, justification, estimate);
        (message, weights)
    }
    fn equivocates(&self, rhs: &Self) -> bool {
        self != rhs
            && self.get_sender() == rhs.get_sender()
            && !rhs.depends(self)
            && !self.depends(rhs)
    }
    fn depends(&self, rhs: &Self) -> bool {
        // although the recursion ends supposedly only at genesis message, the
        // trick is the following: it short-circuits while descending on the
        // dependency tree, if it finds a dependent message. when dealing with
        // honest validators, this would return true very fast. all the new
        // derived branches of the justification will be evaluated in parallel.
        // say if a msg is justified by 2 other msgs, then the 2 other msgs will
        // be processed on different threads. this applies recursively, so if
        // each of the 2 msgs have say 3 justifications, then each of the 2
        // threads will spawn 3 new threads to process each of the messages.
        // thus, highly parallelizable. when it shortcuts, because in one thread
        // a dependency was found, the function returns true and all the
        // computation on the other threads will be canceled.
        let justification = self.get_justification();
        justification.contains(rhs)
            || justification
                .par_iter()
                .any(|self_prime| self_prime.depends(rhs))
    }
    fn is_circular(&self, rhs: &Self) -> bool {
        rhs.depends(self) && self.depends(rhs)
    }

    /// returns the dag tip-most safe messages. a safe message is defined as one
    /// that was seen by all senders (with non-zero weight in senders_weights)
    /// and all senders saw each other seeing each other messages.
    fn get_finalized_msgs(
        &self,
        all_senders: &HashSet<Self::Sender>,
    ) -> HashSet<Self> {
        fn recursor<M>(
            m: &M,
            all_senders: &HashSet<M::Sender>,
            mut senders_referred: HashSet<M::Sender>,
            safe_ms: HashSet<M>,
            original_sender: &M::Sender,
        ) -> HashSet<M>
        where
            M: AbstractMsg,
        {
            m.get_justification().iter().fold(
                safe_ms,
                |mut safe_ms_prime, m_prime| {
                    // base case
                    if &senders_referred == all_senders
                        && original_sender == m_prime.get_sender()
                    {
                        let _ = safe_ms_prime.insert(m_prime.clone());
                        safe_ms_prime
                    }
                    else {
                        let _ = senders_referred
                            .insert(m_prime.get_sender().clone());
                        recursor(
                            m_prime,
                            all_senders,
                            senders_referred.clone(),
                            safe_ms_prime,
                            original_sender,
                        )
                    }
                },
            )
        };
        // initial state, trigger recursion
        let original_sender = self.get_sender();
        let senders_refered =
            [original_sender.clone()].iter().cloned().collect();
        let safe_msgs = HashSet::new();
        recursor(
            self,
            all_senders,
            senders_refered,
            safe_msgs,
            &original_sender,
        )
    }

    /// returns the dag tip-most safe messages. a safe message is defined as one
    /// that was seen by more than a given thr of total weight units. TODO: not
    /// sure about this implementation, i assume its not working correctly.
    fn get_safe_msgs_by_weight(
        &self,
        senders_weights: &SendersWeight<Self::Sender>,
        thr: &WeightUnit,
    ) -> HashMap<Self, WeightUnit> {
        fn recursor<M>(
            m: &M,
            senders_weights: &SendersWeight<M::Sender>,
            mut senders_referred: HashSet<M::Sender>,
            weight_referred: WeightUnit,
            thr: &WeightUnit,
            safe_msg_weight: HashMap<M, WeightUnit>,
        ) -> HashMap<M, WeightUnit>
        where
            M: AbstractMsg,
        {
            m.get_justification().iter().fold(
                safe_msg_weight,
                |mut safe_prime, m_prime| {
                    // base case
                    if &weight_referred > thr {
                        let _ = safe_prime.insert(m.clone(), weight_referred);
                        safe_prime
                    }
                    else {
                        let sender_current = m_prime.get_sender();
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
                            senders_weights,
                            senders_referred.clone(),
                            weight_referred,
                            thr,
                            safe_prime,
                        )
                    }
                },
            )
        };
        // initial state, trigger recursion
        let senders_referred = [].iter().cloned().collect();
        let weight_referred = WeightUnit::ZERO;
        let safe_msg_weight = HashMap::new();
        recursor(
            self,
            senders_weights,
            senders_referred,
            weight_referred,
            thr,
            safe_msg_weight,
        )
    }
}

#[derive(Clone, Default, Eq, PartialEq, PartialOrd, Ord)]
struct ProtoMessage<E, S>
where
    E: Estimate<M = Message<E, S>>,
    S: Sender,
    Message<E, S>: Data,
{
    estimate: E,
    sender: S,
    justification: Justification<Message<E, S>>,
}

#[derive(Eq, Ord, PartialOrd, Clone, Default)]
pub struct Message<E, S>(Arc<ProtoMessage<E, S>>)
where
    E: Estimate<M = Message<E, S>>,
    S: Sender,
    Message<E, S>: Data;

/*
// TODO start we should make messages lazy. continue this after async-await is better
// documented

enum MsgStatus {
    Unchecked,
    Valid,
    Invalid,
}

struct Msg<E, S, D>
where
    E: Estimate,
    S: Sender,
{
    status: MsgStatus,
    msg: Future<Item = Message<E, S, D>, Error = Error>,
}
// TODO end
*/

impl<E, S> AbstractMsg for Message<E, S>
where
    E: Estimate<M = Self>,
    S: Sender,
    Message<E, S>: Data,
{
    type Estimate = E;
    type Sender = S;

    fn get_sender(&self) -> &Self::Sender {
        &self.0.sender
    }
    fn get_estimate(&self) -> &Self::Estimate {
        &self.0.estimate
    }
    fn get_justification<'z>(&'z self) -> &'z Justification<Self> {
        &self.0.justification
    }

    fn new(sender: S, justification: Justification<Self>, estimate: E) -> Self {
        Message(Arc::new(ProtoMessage {
            sender,
            justification,
            estimate,
        }))
    }
}

impl<E, S> Hash for Message<E, S>
where
    E: Estimate<M = Self>,
    S: Sender,
    Message<E, S>: Data,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        let _ = self.get_sender().hash(state);
        let _ = self.get_justification().hash(state);
        let _ = self.get_estimate().hash(state); // the hash of the msg does depend on the estimate
    }
}

impl<E, S> PartialEq for Message<E, S>
where
    E: Estimate<M = Self>,
    S: Sender,
    Message<E, S>: Data,
{
    fn eq(&self, rhs: &Self) -> bool {
        self.get_sender() == rhs.get_sender()
            && self.get_justification() == rhs.get_justification()
            && self.get_estimate() == rhs.get_estimate()
    }
}

impl<E, S> Debug for Message<E, S>
where
    E: Estimate<M = Self>,
    S: Sender,
    Message<E, S>: Data,
{
    fn fmt(&self, f: &mut Formatter) -> Result {
        write!(
            f,
            "M{:?}({:?}) -> {:?}",
            self.get_sender(),
            self.get_estimate().clone(),
            self.get_justification()
        )
    }
}

#[cfg(test)]
mod message {
    use vote_count::{VoteCount};
    use senders_weight::{SendersWeight};

    use std::{f64};
    use super::*;
    #[test]
    fn debug() {
        let v0 = VoteCount::create_vote_msg(0, false);
        println!("{:?}", v0);
    }

    #[test]
    fn msg_equality() {
        // v0 and v0_prime are equivocating messages (first child of a fork).
        let v0 = &VoteCount::create_vote_msg(0, false);
        let v1 = &VoteCount::create_vote_msg(1, true);
        let v0_prime = &VoteCount::create_vote_msg(0, true); // equivocating vote
        let v0_idem = &VoteCount::create_vote_msg(0, false);
        assert!(v0 == v0_idem, "v0 and v0_idem should be equal");
        assert!(v0 != v0_prime, "v0 and v0_prime should NOT be equal");
        assert!(v0 != v1, "v0 and v1 should NOT be equal");

        let senders_weights = SendersWeight::new(
            [(0, 1.0), (1, 1.0), (2, 1.0)].iter().cloned().collect(),
        );
        let weights = Weights::new(senders_weights, 0.0, 0.0, HashSet::new());
        let mut j0 = Justification::new();
        assert!(
            j0.faulty_inserts(vec![v0].iter().cloned().collect(), &weights)
                .success
        );

        let external_data: Option<VoteCount> = None;
        let (m0, _) =
            &Message::from_msgs(0, vec![v0], None, &weights, external_data);
        // let m0 = &Message::new(0, justification, estimate);

        let mut j1 = Justification::new();
        assert!(
            j1.faulty_inserts(vec![v0].iter().cloned().collect(), &weights)
                .success
        );
        assert!(
            j1.faulty_inserts(vec![m0].iter().cloned().collect(), &weights)
                .success
        );

        let (msg1, _) = Message::from_msgs(0, vec![v0], None, &weights, None);
        let (msg2, _) = Message::from_msgs(0, vec![v0], None, &weights, None);
        assert!(msg1 == msg2, "messages should be equal");
        let (msg3, _) =
            Message::from_msgs(0, vec![v0, m0], None, &weights, None);
        assert!(msg1 != msg3, "msg1 should be different than msg3");
    }

    #[test]
    fn msg_depends() {
        let v0 = &VoteCount::create_vote_msg(0, false);
        let v0_prime = &VoteCount::create_vote_msg(0, true); // equivocating vote

        let senders_weights = SendersWeight::new(
            [(0, 1.0), (1, 1.0), (2, 1.0)].iter().cloned().collect(),
        );
        let weights = Weights::new(senders_weights, 0.0, 0.0, HashSet::new());

        let mut j0 = Justification::new();
        assert!(
            j0.faulty_inserts(vec![v0].iter().cloned().collect(), &weights)
                .success
        );
        let (m0, _) = &Message::from_msgs(0, vec![v0], None, &weights, None);
        assert!(
            !v0.depends(v0_prime),
            "v0 does NOT depends on v0_prime as they are equivocating"
        );
        assert!(
            !m0.depends(m0),
            "m0 does NOT depends on itself directly, by our impl choice"
        );
        assert!(!m0.depends(v0_prime), "m0 depends on v0 directly");
        assert!(m0.depends(v0), "m0 depends on v0 directly");

        let mut j0 = Justification::new();
        assert!(
            j0.faulty_inserts([v0].iter().cloned().collect(), &weights)
                .success
        );
        let (m0, _) = &Message::from_msgs(0, vec![v0], None, &weights, None);

        let mut j1 = Justification::new();
        assert!(
            j1.faulty_inserts([v0].iter().cloned().collect(), &weights)
                .success
        );
        assert!(
            j1.faulty_inserts([m0].iter().cloned().collect(), &weights)
                .success
        );

        let (m1, _) =
            &Message::from_msgs(0, vec![v0, m0], None, &weights, None);
        assert!(m1.depends(m0), "m1 DOES depent on m0");
        assert!(!m0.depends(m1), "but m0 does NOT depend on m1");
        assert!(m1.depends(v0), "m1 depends on v0 through m0");
    }

    #[test]
    fn msg_equivocates() {
        let v0 = &VoteCount::create_vote_msg(0, false);
        let v0_prime = &VoteCount::create_vote_msg(0, true); // equivocating vote
        let v1 = &VoteCount::create_vote_msg(1, true);

        let senders_weights = SendersWeight::new(
            [(0, 1.0), (1, 1.0), (2, 1.0)].iter().cloned().collect(),
        );
        let weights = Weights::new(senders_weights, 0.0, 0.0, HashSet::new());

        let mut j0 = Justification::new();
        assert!(
            j0.faulty_inserts(vec![v0].iter().cloned().collect(), &weights)
                .success
        );
        let (m0, _) = &Message::from_msgs(0, vec![v0], None, &weights, None);
        assert!(!v0.equivocates(v0), "should be all good");
        assert!(!v1.equivocates(m0), "should be all good");
        assert!(!m0.equivocates(v1), "should be all good");
        assert!(v0.equivocates(v0_prime), "should be a direct equivocation");
        assert!(
            m0.equivocates(v0_prime),
            "should be an indirect equivocation, equivocates to m0 through v0"
        );
    }
    #[test]
    fn msg_safe_by_weight() {
        let sender0 = 0;
        let sender1 = 1;
        let senders_weights = &SendersWeight::new(
            [(sender0, 1.0), (sender1, 1.0)].iter().cloned().collect(),
        );

        let relative_senders_weights = &senders_weights.into_relative_weights();
        let weights =
            Weights::new(senders_weights.clone(), 0.0, 0.0, HashSet::new());

        let thr = &0.5;
        // sender0        v0---m0        m2---
        // sender1               \--m1--/
        let v0 = &VoteCount::create_vote_msg(sender0, false);
        let safe_msgs =
            v0.get_safe_msgs_by_weight(relative_senders_weights, thr);
        assert_eq!(safe_msgs.len(), 0, "only 0.5 of weight saw v0");

        let (m0, _) = &Message::from_msgs(
            sender0,
            vec![&v0],
            None,
            &weights,
            None as Option<VoteCount>,
        );
        let safe_msgs =
            m0.get_safe_msgs_by_weight(relative_senders_weights, thr);
        assert_eq!(safe_msgs.len(), 0, "only 0.5 of weight saw v0 and m0");

        let (m1, _) = &Message::from_msgs(
            sender1,
            vec![&m0],
            None,
            &weights,
            None as Option<VoteCount>,
        );
        let safe_msgs =
            m1.get_safe_msgs_by_weight(relative_senders_weights, thr);
        assert_eq!(
            safe_msgs.len(),
            0,
            "both sender0 (0.5) and sender1 (0.5) saw v0 and m0, but sender0 hasn't
necessarly seen sender1 seeing v0 and m0, thus not yet safe"
        );

        let (m2, _) = &Message::from_msgs(
            sender0,
            vec![&m1],
            None,
            &weights,
            None as Option<VoteCount>,
        );
        let safe_msgs =
            m2.get_safe_msgs_by_weight(relative_senders_weights, thr);
        assert_eq!(
            safe_msgs.get(m0).unwrap_or(&f64::NAN),
            &1.0,
            "both sender0 and sender1 saw v0 and m0, and additionally both
parties saw each other seing v0 and m0, m0 (and all its dependencies) are final"
        );
        // let senders = &Sender::get_senders(&relative_senders_weights);
    }

    #[test]
    fn msg_safe_by_sender() {
        // setup
        let sender0 = 0;
        let sender1 = 1;
        let senders_weights = SendersWeight::new(
            [(sender0, 1.0), (sender1, 1.0)].iter().cloned().collect(),
        );
        let weights =
            Weights::new(senders_weights.clone(), 0.0, 0.0, HashSet::new());
        let senders = &senders_weights.get_senders();

        // sender0        v0---m0        m2---
        // sender1               \--m1--/
        let v0 = &VoteCount::create_vote_msg(sender0, false);
        let safe_msgs = v0.get_finalized_msgs(senders);
        assert_eq!(safe_msgs.len(), 0, "only sender0 saw v0");

        let (m0, weights) = &Message::from_msgs(
            sender0,
            vec![v0],
            None,
            &weights,
            None as Option<VoteCount>,
        );
        let safe_msgs = m0.get_finalized_msgs(senders);
        assert_eq!(safe_msgs.len(), 0, "only sender0 saw v0 and m0");

        let (m1, weights) = &Message::from_msgs(
            sender1,
            vec![m0],
            None,
            &weights,
            None as Option<VoteCount>,
        );
        let safe_msgs = m1.get_finalized_msgs(senders);
        assert_eq!(
            safe_msgs.len(),
            0,
            "both sender0 and sender1 saw v0 and m0, but sender0 hasn't
necessarly seen sender1 seeing v0 and m0, thus not yet safe"
        );

        let (m2, weights) = &Message::from_msgs(
            sender0,
            vec![m1],
            None,
            &weights,
            None as Option<VoteCount>,
        );
        let safe_msgs = m2.get_finalized_msgs(senders);
        assert_eq!(
            safe_msgs,
            [m0.clone()].iter().cloned().collect(),
            "both sender0 and sender1 saw v0 and m0, and additionally both
parties saw each other seing v0 and m0, m0 (and all its dependencies) are final"
        );

        // sender0        v0---m0        m2---
        // sender1        v1--/   \--m1--/
        let v0 = &VoteCount::create_vote_msg(sender0, false);
        let v1 = &VoteCount::create_vote_msg(sender1, false);
        let (m0, weights) = &Message::from_msgs(
            sender0,
            vec![v0, v1],
            None,
            &weights,
            None as Option<VoteCount>,
        );
        let safe_msgs = m0.get_finalized_msgs(senders);
        assert_eq!(
            safe_msgs.len(),
            0,
            "sender0 saw v0, v1 and m0, and sender1 saw only v1"
        );

        let (m1, weights) = &Message::from_msgs(
            sender1,
            vec![m0],
            None,
            &weights,
            None as Option<VoteCount>,
        );
        let safe_msgs = m1.get_finalized_msgs(senders);

        assert_eq!(
            safe_msgs,
            [v1.clone()].iter().cloned().collect(),
            "both sender0 and sender1 saw v0, v1 and m0, but sender0 hasn't
necessarly seen sender1 seeing v0 and m0, just v1 is safe"
        );

        let (m2, _weights) = &Message::from_msgs(
            sender0,
            vec![m1],
            None,
            &weights,
            None as Option<VoteCount>,
        );
        let safe_msgs = m2.get_finalized_msgs(senders);
        assert_eq!(
            safe_msgs,
            [m0.clone()].iter().cloned().collect(),
            "both sender0 and sender1 saw v0 and m0, and additionally both
parties saw each other seing v0 and m0, safe"
        );
    }
}
