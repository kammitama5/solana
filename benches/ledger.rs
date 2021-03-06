#![feature(test)]
extern crate solana;
extern crate test;

use solana::hash::{hash, Hash};
use solana::ledger::{next_entries, reconstruct_entries_from_blobs, Block};
use solana::packet::BlobRecycler;
use solana::signature::{Keypair, KeypairUtil};
use solana::transaction::Transaction;
use std::collections::VecDeque;
use test::Bencher;

#[bench]
fn bench_block_to_blobs_to_block(bencher: &mut Bencher) {
    let zero = Hash::default();
    let one = hash(&zero.as_ref());
    let keypair = Keypair::new();
    let tx0 = Transaction::new(&keypair, keypair.pubkey(), 1, one);
    let transactions = vec![tx0; 10];
    let entries = next_entries(&zero, 1, transactions);

    let blob_recycler = BlobRecycler::default();
    bencher.iter(|| {
        let mut blob_q = VecDeque::new();
        entries.to_blobs(&blob_recycler, &mut blob_q);
        assert_eq!(reconstruct_entries_from_blobs(blob_q).unwrap(), entries);
    });
}
