//! The `record_stage` module provides an object for generating a Proof of History.
//! It records Transaction items on behalf of its users. It continuously generates
//! new hashes, only stopping to check if it has been sent an Transaction item. It
//! tags each Transaction with an Entry, and sends it back. The Entry includes the
//! Transaction, the latest hash, and the number of hashes since the last transaction.
//! The resulting stream of entries represents ordered transactions in time.

use entry::Entry;
use hash::Hash;
use recorder::Recorder;
use service::Service;
use std::sync::mpsc::{channel, Receiver, RecvError, Sender, TryRecvError};
use std::thread::{self, Builder, JoinHandle};
use std::time::{Duration, Instant};
use transaction::Transaction;

#[cfg_attr(feature = "cargo-clippy", allow(large_enum_variant))]
pub enum Signal {
    Tick,
    Transactions(Vec<Transaction>),
}

pub struct RecordStage {
    thread_hdl: JoinHandle<()>,
}

impl RecordStage {
    /// A background thread that will continue tagging received Transaction messages and
    /// sending back Entry messages until either the receiver or sender channel is closed.
    pub fn new(
        signal_receiver: Receiver<Signal>,
        start_hash: &Hash,
    ) -> (Self, Receiver<Vec<Entry>>) {
        let (entry_sender, entry_receiver) = channel();
        let start_hash = *start_hash;

        let thread_hdl = Builder::new()
            .name("solana-record-stage".to_string())
            .spawn(move || {
                let mut recorder = Recorder::new(start_hash);
                let _ = Self::process_signals(&mut recorder, &signal_receiver, &entry_sender);
            })
            .unwrap();

        (RecordStage { thread_hdl }, entry_receiver)
    }

    /// Same as `RecordStage::new`, but will automatically produce entries every `tick_duration`.
    pub fn new_with_clock(
        signal_receiver: Receiver<Signal>,
        start_hash: &Hash,
        tick_duration: Duration,
    ) -> (Self, Receiver<Vec<Entry>>) {
        let (entry_sender, entry_receiver) = channel();
        let start_hash = *start_hash;

        let thread_hdl = Builder::new()
            .name("solana-record-stage".to_string())
            .spawn(move || {
                let mut recorder = Recorder::new(start_hash);
                let start_time = Instant::now();
                loop {
                    if Self::try_process_signals(
                        &mut recorder,
                        start_time,
                        tick_duration,
                        &signal_receiver,
                        &entry_sender,
                    ).is_err()
                    {
                        return;
                    }
                    recorder.hash();
                }
            })
            .unwrap();

        (RecordStage { thread_hdl }, entry_receiver)
    }

    fn process_signal(
        signal: Signal,
        recorder: &mut Recorder,
        sender: &Sender<Vec<Entry>>,
    ) -> Result<(), ()> {
        let txs = if let Signal::Transactions(txs) = signal {
            txs
        } else {
            vec![]
        };
        let entries = recorder.record(txs);
        sender.send(entries).or(Err(()))?;
        Ok(())
    }

    fn process_signals(
        recorder: &mut Recorder,
        receiver: &Receiver<Signal>,
        sender: &Sender<Vec<Entry>>,
    ) -> Result<(), ()> {
        loop {
            match receiver.recv() {
                Ok(signal) => Self::process_signal(signal, recorder, sender)?,
                Err(RecvError) => return Err(()),
            }
        }
    }

    fn try_process_signals(
        recorder: &mut Recorder,
        start_time: Instant,
        tick_duration: Duration,
        receiver: &Receiver<Signal>,
        sender: &Sender<Vec<Entry>>,
    ) -> Result<(), ()> {
        loop {
            if let Some(entry) = recorder.tick(start_time, tick_duration) {
                sender.send(vec![entry]).or(Err(()))?;
            }
            match receiver.try_recv() {
                Ok(signal) => Self::process_signal(signal, recorder, sender)?,
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => return Err(()),
            };
        }
    }
}

impl Service for RecordStage {
    fn thread_hdls(self) -> Vec<JoinHandle<()>> {
        vec![self.thread_hdl]
    }

    fn join(self) -> thread::Result<()> {
        self.thread_hdl.join()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ledger::Block;
    use signature::{Keypair, KeypairUtil};
    use std::sync::mpsc::channel;
    use std::thread::sleep;

    #[test]
    fn test_historian() {
        let (tx_sender, tx_receiver) = channel();
        let zero = Hash::default();
        let (record_stage, entry_receiver) = RecordStage::new(tx_receiver, &zero);

        tx_sender.send(Signal::Tick).unwrap();
        sleep(Duration::new(0, 1_000_000));
        tx_sender.send(Signal::Tick).unwrap();
        sleep(Duration::new(0, 1_000_000));
        tx_sender.send(Signal::Tick).unwrap();

        let entry0 = entry_receiver.recv().unwrap()[0].clone();
        let entry1 = entry_receiver.recv().unwrap()[0].clone();
        let entry2 = entry_receiver.recv().unwrap()[0].clone();

        assert_eq!(entry0.num_hashes, 0);
        assert_eq!(entry1.num_hashes, 0);
        assert_eq!(entry2.num_hashes, 0);

        drop(tx_sender);
        assert_eq!(record_stage.thread_hdl.join().unwrap(), ());

        assert!([entry0, entry1, entry2].verify(&zero));
    }

    #[test]
    fn test_historian_closed_sender() {
        let (tx_sender, tx_receiver) = channel();
        let zero = Hash::default();
        let (record_stage, entry_receiver) = RecordStage::new(tx_receiver, &zero);
        drop(entry_receiver);
        tx_sender.send(Signal::Tick).unwrap();
        assert_eq!(record_stage.thread_hdl.join().unwrap(), ());
    }

    #[test]
    fn test_transactions() {
        let (tx_sender, signal_receiver) = channel();
        let zero = Hash::default();
        let (_record_stage, entry_receiver) = RecordStage::new(signal_receiver, &zero);
        let alice_keypair = Keypair::new();
        let bob_pubkey = Keypair::new().pubkey();
        let tx0 = Transaction::new(&alice_keypair, bob_pubkey, 1, zero);
        let tx1 = Transaction::new(&alice_keypair, bob_pubkey, 2, zero);
        tx_sender
            .send(Signal::Transactions(vec![tx0, tx1]))
            .unwrap();
        drop(tx_sender);
        let entries: Vec<_> = entry_receiver.iter().collect();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_clock() {
        let (tx_sender, tx_receiver) = channel();
        let zero = Hash::default();
        let (_record_stage, entry_receiver) =
            RecordStage::new_with_clock(tx_receiver, &zero, Duration::from_millis(20));
        sleep(Duration::from_millis(900));
        tx_sender.send(Signal::Tick).unwrap();
        drop(tx_sender);
        let entries: Vec<_> = entry_receiver.iter().flat_map(|x| x).collect();
        assert!(entries.len() > 1);

        // Ensure the ID is not the seed.
        assert_ne!(entries[0].id, zero);
    }
}
