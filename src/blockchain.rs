use std::collections::{BTreeSet, HashSet};
use traits::{Estimate, Data};
use message::{AbstractMsg, Message};
use justification::{Justification, Weights};
use senders_weight::{SendersWeight};


type Validator = u32;

/// a genesis block should be a block with estimate Blockchain with prevblock =
/// None and data. data will be the unique identifier of this blockchain
#[derive(Clone, Default, Eq, PartialEq, PartialOrd, Ord, Debug, Hash)]
pub struct Blockchain {
    prevblock: Option<Block>,
    data: Option<<Block as Data>::Data>, // TODO: lift the Option when we have real data structures.
}

pub type Block =
    Message<Blockchain /*Estimate*/, Validator /*Sender*/>;

#[derive(Clone, Eq, Debug, Ord, PartialOrd, PartialEq, Hash)]
pub struct Tx;

impl Data for Block {
    type Data = BTreeSet<Tx>;
    fn is_valid(_data: &Self::Data) -> bool {
        unimplemented!()
    }
}
impl Blockchain {
    pub fn new(
        prevblock: Option<Block>,
        data: Option<<Block as Data>::Data>,
    ) -> Self {
        Self { prevblock, data }
    }
    fn get_prevblock(&self) -> &Option<Block> {
        &self.prevblock
    }
    fn get_data(&self) -> &Option<<Block as Data>::Data> {
        &self.data
    }
}

impl Block {
    fn get_prevblock(&self) -> Option<&Self> {
        self.get_estimate().get_prevblock().as_ref()
    }

    fn is_member(&self, rhs: &Self) -> bool {
        self == rhs
            || rhs
                .get_prevblock()
                .map(|prevblock| self.is_member(prevblock))
                .unwrap_or(false)
    }
}

// TODO: i think its possible to do ghost in arbitrary data, that is default implementation
impl Estimate for Blockchain {
    type M = Block;
    fn mk_estimate(
        latest_msgs: &Justification<Self::M>,
        finalized_msg: Option<&Self::M>,
        weights: &Weights<<<Self as Estimate>::M as AbstractMsg>::Sender>,
        data: Option<<<Self as Estimate>::M as Data>::Data>,
    ) -> Self {
        match latest_msgs.len() {
            0 => panic!(
                "Needs at least one latest message to be able to pick one"
            ),
            1 => Self::new(latest_msgs.iter().next().cloned(), data),
            _ => {
                let heaviest_subtree = latest_msgs
                    .ghost(finalized_msg, weights.get_senders_weights());
                Self::new(heaviest_subtree, data)
            },
        }
    }
}

#[test]
fn example_usage() {
    let (sender0, sender1, sender2, sender3) = (0, 1, 2, 3); // miner identities
    let (weight0, weight1, weight2, weight3) = (5.0, 2.0, 2.0, 1.0); // and their corresponding weights
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
    let weights = Weights::new(
        senders_weights.clone(),
        0.0,            // state fault weight
        1.0,            // subjective fault weight threshold
        HashSet::new(), // equivocators
    );

    let estimate = Blockchain {
        prevblock: None,
        data: None,
    };
    let justification = Justification::new();
    let genesis_block = Block::new(sender0, justification, estimate.clone());
    assert_eq!(
        genesis_block.get_estimate(),
        &estimate,
        "genesis block with None as prevblock"
    );

    let (b1, weights) = Block::from_msgs(
        sender1,
        vec![&genesis_block],
        None, // finalized_msg, could be genesis_block
        &weights,
        None, // data
    );

    let (b2, weights) =
        Block::from_msgs(sender2, vec![&genesis_block], None, &weights, None);

    let (b3, weights) =
        Block::from_msgs(sender3, vec![&b1, &b2], None, &weights, None);
    
    assert_eq!(
        b3.get_estimate(),
        &Blockchain::new(Some(b2.clone()), None),
        "should build on top of b2"
    );
    
    assert!(b1.is_member(&b1), "equal blocks");
    assert!(!b1.is_member(&b2));
    assert!(!b2.is_member(&b1));
    assert!(!b1.is_member(&b2));
    assert!(b2.is_member(&b3));
    assert!(!b3.is_member(&b2));
    assert!(!b3.is_member(&b1));
}

#[test]
fn example_equal_weight(){
    let (sender0, sender1, sender2, sender3, sender4) = (0, 1, 2, 3, 4); // miner identities
    let (weight0, weight1, weight2, weight3, weight4) = (1.0, 1.0, 1.0, 1.0, 1.0); // and their corresponding weights
    let senders_weights = SendersWeight::new(
        [
            (sender0, weight0),
            (sender1, weight1),
            (sender2, weight2),
            (sender3, weight3),
            (sender4, weight4),
        ].iter()
            .cloned()
            .collect(),
    );
    let weights = Weights::new(
        senders_weights.clone(),
        0.0,            // state fault weight
        1.0,            // subjective fault weight threshold
        HashSet::new(), // equivocators
    );

    let estimate = Blockchain {
        prevblock: None,
        data: None,
    };
    let justification = Justification::new();
    let genesis_block = Block::new(sender0, justification, estimate.clone());
    assert_eq!(
        genesis_block.get_estimate(),
        &estimate,
        "genesis block with None as prevblock"
    );

    let (b1, weights) = Block::from_msgs(
        sender1,
        vec![&genesis_block],
        None, // finalized_msg, could be genesis_block
        &weights,
        None, // data
    );

    let (b2, weights) =
        Block::from_msgs(sender2, vec![&genesis_block], None, &weights, None);
    let (b3, weights) = 
        Block::from_msgs(sender3, vec![&genesis_block], None, &weights, None);
    let (b4, weights) =
        Block::from_msgs(sender4, vec![&genesis_block], None, &weights, None);
    let (b5, weights) =
        Block::from_msgs(sender1, vec![&b1, &b2], None, &weights, None);
    let (b6, weights) =
        Block::from_msgs(sender2, vec![&b2], None, &weights, None);
    let (b7, weights) =
        Block::from_msgs(sender3, vec![&b3], None, &weights, None);
    let (b8, weights) =
        Block::from_msgs(sender4, vec![&b3, &b4], None, &weights, None);
    let (b9, weights) =
        Block::from_msgs(sender1, vec![&b5], None, &weights, None);
    let (b10, weights) =
        Block::from_msgs(sender2, vec![&b5], None, &weights, None);
    let (b11, weights) =
        Block::from_msgs(sender3, vec![&b8], None, &weights, None);
    let (b12, weights) =
        Block::from_msgs(sender4, vec![&b8], None, &weights, None);

    
    assert_eq!(
        b5.get_estimate(),
        &Blockchain::new(Some(b2.clone()), None),
        "should build on top of b2"
    );

    assert!(b1.is_member(&b1), "equal blocks");
    assert!(!b1.is_member(&b2));
    assert!(!b2.is_member(&b1));
    assert!(!b1.is_member(&b2));
    assert!(b3.is_member(&b8));
    assert!(!b3.is_member(&b2));
    assert!(!b3.is_member(&b1));
    assert!(!b3.is_member(&b2));
}