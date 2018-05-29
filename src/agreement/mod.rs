//! Binary Byzantine agreement protocol from a common coin protocol.

pub mod bin_values;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Debug;
use std::hash::Hash;
use std::mem::replace;
use std::rc::Rc;

use itertools::Itertools;

use agreement::bin_values::BinValues;
use messaging::{DistAlgorithm, NetworkInfo, Target, TargetedMessage};

error_chain!{
    types {
        Error, ErrorKind, ResultExt, AgreementResult;
    }

    errors {
        InputNotAccepted
        Terminated
    }
}

#[cfg_attr(feature = "serialization-serde", derive(Serialize))]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AgreementContent {
    /// `BVal` message.
    BVal(bool),
    /// `Aux` message.
    Aux(bool),
    /// `Conf` message.
    Conf(BinValues),
}

/// Messages sent during the binary Byzantine agreement stage.
#[cfg_attr(feature = "serialization-serde", derive(Serialize))]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct AgreementMessage {
    pub epoch: u32,
    pub content: AgreementContent,
}

impl AgreementMessage {
    pub fn bval(epoch: u32, b: bool) -> Self {
        AgreementMessage {
            epoch,
            content: AgreementContent::BVal(b),
        }
    }

    pub fn aux(epoch: u32, b: bool) -> Self {
        AgreementMessage {
            epoch,
            content: AgreementContent::Aux(b),
        }
    }

    pub fn conf(epoch: u32, v: BinValues) -> Self {
        AgreementMessage {
            epoch,
            content: AgreementContent::Conf(v),
        }
    }
}

/// Binary Agreement instance
pub struct Agreement<NodeUid> {
    /// Shared network information.
    netinfo: Rc<NetworkInfo<NodeUid>>,
    /// Agreement algorithm epoch.
    epoch: u32,
    /// Bin values. Reset on every epoch update.
    bin_values: BinValues,
    /// Values received in `BVal` messages. Reset on every epoch update.
    received_bval: BTreeMap<NodeUid, BTreeSet<bool>>,
    /// Sent `BVal` values. Reset on every epoch update.
    sent_bval: BTreeSet<bool>,
    /// Values received in `Aux` messages. Reset on every epoch update.
    received_aux: BTreeMap<NodeUid, bool>,
    /// Received `Conf` messages. Reset on every epoch update.
    received_conf: BTreeMap<NodeUid, BinValues>,
    /// The estimate of the decision value in the current epoch.
    estimated: Option<bool>,
    /// The value output by the agreement instance. It is set once to `Some(b)`
    /// and then never changed. That is, no instance of Binary Agreement can
    /// decide on two different values of output.
    output: Option<bool>,
    /// A permanent, latching copy of the output value. This copy is required because `output` can
    /// be consumed using `DistAlgorithm::next_output` immediately after the instance finishing to
    /// handle a message, in which case it would otherwise be unknown whether the output value was
    /// ever there at all. While the output value will still be required in a later epoch to decide
    /// the termination state.
    decision: Option<bool>,
    /// A cache for messages for future epochs that cannot be handled yet.
    // TODO: Find a better solution for this; defend against spam.
    incoming_queue: Vec<(NodeUid, AgreementMessage)>,
    /// Termination flag. The Agreement instance doesn't terminate immediately
    /// upon deciding on the agreed value. This is done in order to help other
    /// nodes decide despite asynchrony of communication. Once the instance
    /// determines that all the remote nodes have reached agreement, it sets the
    /// `terminated` flag and accepts no more incoming messages.
    terminated: bool,
    /// The outgoing message queue.
    messages: VecDeque<AgreementMessage>,
    /// Whether the `Conf` message round has started in the current epoch.
    conf_round: bool,
}

impl<NodeUid: Clone + Debug + Eq + Hash + Ord> DistAlgorithm for Agreement<NodeUid> {
    type NodeUid = NodeUid;
    type Input = bool;
    type Output = bool;
    type Message = AgreementMessage;
    type Error = Error;

    fn input(&mut self, input: Self::Input) -> AgreementResult<()> {
        self.set_input(input)
    }

    /// Receive input from a remote node.
    fn handle_message(
        &mut self,
        sender_id: &Self::NodeUid,
        message: Self::Message,
    ) -> AgreementResult<()> {
        if self.terminated {
            return Err(ErrorKind::Terminated.into());
        }
        if message.epoch < self.epoch {
            return Ok(()); // Message is obsolete: We are already in a later epoch.
        }
        if message.epoch > self.epoch {
            // Message is for a later epoch. We can't handle that yet.
            self.incoming_queue.push((sender_id.clone(), message));
            return Ok(());
        }
        match message.content {
            AgreementContent::BVal(b) => self.handle_bval(sender_id, b),
            AgreementContent::Aux(b) => self.handle_aux(sender_id, b),
            AgreementContent::Conf(v) => self.handle_conf(sender_id, v),
        }
    }

    /// Take the next Agreement message for multicast to all other nodes.
    fn next_message(&mut self) -> Option<TargetedMessage<Self::Message, Self::NodeUid>> {
        self.messages
            .pop_front()
            .map(|msg| Target::All.message(msg))
    }

    /// Consume the output. Once consumed, the output stays `None` forever.
    fn next_output(&mut self) -> Option<Self::Output> {
        self.output.take()
    }

    /// Whether the algorithm has terminated.
    fn terminated(&self) -> bool {
        self.terminated
    }

    fn our_id(&self) -> &Self::NodeUid {
        self.netinfo.our_uid()
    }
}

impl<NodeUid: Clone + Debug + Eq + Hash + Ord> Agreement<NodeUid> {
    pub fn new(netinfo: Rc<NetworkInfo<NodeUid>>) -> Self {
        Agreement {
            netinfo,
            epoch: 0,
            bin_values: BinValues::new(),
            received_bval: BTreeMap::new(),
            sent_bval: BTreeSet::new(),
            received_aux: BTreeMap::new(),
            received_conf: BTreeMap::new(),
            estimated: None,
            output: None,
            decision: None,
            incoming_queue: Vec::new(),
            terminated: false,
            messages: VecDeque::new(),
            conf_round: false,
        }
    }

    /// Sets the input value for agreement.
    pub fn set_input(&mut self, input: bool) -> AgreementResult<()> {
        if self.epoch != 0 || self.estimated.is_some() {
            return Err(ErrorKind::InputNotAccepted.into());
        }
        if self.netinfo.num_nodes() == 1 {
            self.decision = Some(input);
            self.output = Some(input);
            self.terminated = true;
        }

        // Set the initial estimated value to the input value.
        self.estimated = Some(input);
        // Record the input value as sent.
        self.send_bval(input)
    }

    /// Acceptance check to be performed before setting the input value.
    pub fn accepts_input(&self) -> bool {
        self.epoch == 0 && self.estimated.is_none()
    }

    fn handle_bval(&mut self, sender_id: &NodeUid, b: bool) -> AgreementResult<()> {
        self.received_bval
            .entry(sender_id.clone())
            .or_insert_with(BTreeSet::new)
            .insert(b);
        let count_bval = self
            .received_bval
            .values()
            .filter(|values| values.contains(&b))
            .count();

        // upon receiving `BVal(b)` messages from 2f + 1 nodes,
        // bin_values := bin_values ∪ {b}
        if count_bval == 2 * self.netinfo.num_faulty() + 1 {
            let previous_bin_values = self.bin_values;
            let bin_values_changed = self.bin_values.insert(b);

            // wait until bin_values != 0, then multicast `Aux(w)`
            // where w ∈ bin_values
            if previous_bin_values == BinValues::None {
                // Send an `Aux` message at most once per epoch.
                self.send_aux(b)
            } else if bin_values_changed {
                // If the `Conf` round has already started, a change in `bin_values` can lead to its
                // end. Try if it has indeed finished.
                self.try_finish_conf_round()
            } else {
                Ok(())
            }
        } else if count_bval == self.netinfo.num_faulty() + 1 && !self.sent_bval.contains(&b) {
            // upon receiving `BVal(b)` messages from f + 1 nodes, if
            // `BVal(b)` has not been sent, multicast `BVal(b)`
            self.send_bval(b)
        } else {
            Ok(())
        }
    }

    fn send_bval(&mut self, b: bool) -> AgreementResult<()> {
        // Record the value `b` as sent.
        self.sent_bval.insert(b);
        // Multicast `BVal`.
        self.messages
            .push_back(AgreementMessage::bval(self.epoch, b));
        // Receive the `BVal` message locally.
        let our_uid = &self.netinfo.our_uid().clone();
        self.handle_bval(our_uid, b)
    }

    fn send_conf(&mut self) -> AgreementResult<()> {
        if self.conf_round {
            // Only one `Conf` message is allowed in an epoch.
            return Ok(());
        }

        let v = self.bin_values;
        // Multicast `Conf`.
        self.messages
            .push_back(AgreementMessage::conf(self.epoch, v));
        // Trigger the start of the `Conf` round.
        self.conf_round = true;
        // Receive the `Conf` message locally.
        let our_uid = &self.netinfo.our_uid().clone();
        self.handle_conf(our_uid, v)
    }

    /// Waits until at least (N − f) `Aux` messages have been received, such that
    /// the set of values carried by these messages, vals, are a subset of
    /// bin_values (note that bin_values_r may continue to change as `BVal`
    /// messages are received, thus this condition may be triggered upon arrival
    /// of either an `Aux` or a `BVal` message).
    fn handle_aux(&mut self, sender_id: &NodeUid, b: bool) -> AgreementResult<()> {
        // Perform the `Aux` message round only if a `Conf` round hasn't started yet.
        if self.conf_round {
            return Ok(());
        }
        self.received_aux.insert(sender_id.clone(), b);
        if self.bin_values == BinValues::None {
            return Ok(());
        }
        if self.count_aux() < self.netinfo.num_nodes() - self.netinfo.num_faulty() {
            // Continue waiting for the (N - f) `Aux` messages.
            return Ok(());
        }
        // Start the `Conf` message round.
        self.send_conf()
    }

    fn handle_conf(&mut self, sender_id: &NodeUid, v: BinValues) -> AgreementResult<()> {
        self.received_conf.insert(sender_id.clone(), v);
        self.try_finish_conf_round()
    }

    fn try_finish_conf_round(&mut self) -> AgreementResult<()> {
        if self.conf_round {
            let (count_vals, vals) = self.count_conf();
            if count_vals < self.netinfo.num_nodes() - self.netinfo.num_faulty() {
                // Continue waiting for (N - f) `Conf` messages
                return Ok(());
            }
            self.invoke_coin(vals)
        } else {
            Ok(())
        }
    }

    fn send_aux(&mut self, b: bool) -> AgreementResult<()> {
        // Multicast `Aux`.
        self.messages
            .push_back(AgreementMessage::aux(self.epoch, b));
        // Receive the `Aux` message locally.
        let our_uid = &self.netinfo.our_uid().clone();
        self.handle_aux(our_uid, b)
    }

    /// The count of `Aux` messages such that the set of values carried by those messages is a
    /// subset of bin_values_r.
    ///
    /// In general, we can't expect every good node to send the same `Aux` value, so waiting for N -
    /// f agreeing messages would not always terminate. We can, however, expect every good node to
    /// send an `Aux` value that will eventually end up in our `bin_values`.
    fn count_aux(&self) -> usize {
        self.received_aux
            .values()
            .filter(|&&b| self.bin_values.contains(b))
            .count()
    }

    /// Counts the number of received `Conf` messages.
    fn count_conf(&self) -> (usize, BinValues) {
        let (vals_cnt, vals) = self
            .received_conf
            .values()
            .filter(|&conf| conf.is_subset(self.bin_values))
            .tee();

        (vals_cnt.count(), vals.cloned().collect())
    }

    fn start_next_epoch(&mut self) {
        self.bin_values.clear();
        self.received_bval.clear();
        self.sent_bval.clear();
        self.received_aux.clear();
        self.received_conf.clear();
        self.conf_round = false;
        self.epoch += 1;
    }

    /// Gets a common coin and uses it to compute the next decision estimate and outputs the
    /// optional decision value.  The function may start the next epoch. In that case, it also
    /// returns a message for broadcast.
    fn invoke_coin(&mut self, vals: BinValues) -> AgreementResult<()> {
        debug!(
            "{:?} invoke_coin in epoch {}",
            self.netinfo.our_uid(),
            self.epoch
        );
        // FIXME: Implement the Common Coin algorithm. At the moment the
        // coin value is common across different nodes but not random.
        let coin = (self.epoch % 2) == 0;

        // Check the termination condition: "continue looping until both a
        // value b is output in some round r, and the value Coin_r' = b for
        // some round r' > r."
        self.terminated = self.terminated || self.decision == Some(coin);
        if self.terminated {
            debug!("Agreement instance {:?} terminated", self.netinfo.our_uid());
            return Ok(());
        }

        self.start_next_epoch();
        debug!(
            "Agreement instance {:?} started epoch {}",
            self.netinfo.our_uid(),
            self.epoch
        );

        if let Some(b) = vals.definite() {
            self.estimated = Some(b);
            // Outputting a value is allowed only once.
            if self.decision.is_none() && b == coin {
                // Output the agreement value.
                self.output = Some(b);
                // Latch the decided state.
                self.decision = Some(b);
                debug!(
                    "Agreement instance {:?} output: {}",
                    self.netinfo.our_uid(),
                    b
                );
            }
        } else {
            self.estimated = Some(coin);
        }

        let b = self.estimated.unwrap();
        self.send_bval(b)?;
        let queued_msgs = replace(&mut self.incoming_queue, Vec::new());
        for (sender_id, msg) in queued_msgs {
            self.handle_message(&sender_id, msg)?;
        }
        Ok(())
    }
}
