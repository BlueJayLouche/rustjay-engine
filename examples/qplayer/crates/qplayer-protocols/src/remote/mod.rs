//! Remote control protocol — block-transfer show file sync over OSC.
//!
//! Replaces C# `ShowFileSender`.
//!
//! Show files are split into 1 KB blocks and sent as OSC blobs.
//! The receiver ACKs/NACKs individual blocks. Unacknowledged blocks are
//! retransmitted after 250 ms.

use rosc::{OscMessage, OscType};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const BLOCK_SIZE: usize = 1024;
const TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_AFTER: Duration = Duration::from_millis(250);

/// State machine for sending a show file to a remote node.
pub struct ShowFileTransfer {
    target: String,
    blocks: Vec<Block>,
    total_blocks: usize,
    pending: HashMap<usize, Block>,
    start_time: Instant,
    is_complete: bool,
}

#[derive(Debug, Clone)]
struct Block {
    index: usize,
    data: Vec<u8>,
    sent_at: Option<Instant>,
    acknowledged: bool,
}

impl ShowFileTransfer {
    pub fn new(target: String, data: Vec<u8>) -> Self {
        let mut blocks = Vec::new();
        let mut pos = 0;
        let mut idx = 0;
        while pos < data.len() {
            let end = (pos + BLOCK_SIZE).min(data.len());
            blocks.push(Block {
                index: idx,
                data: data[pos..end].to_vec(),
                sent_at: None,
                acknowledged: false,
            });
            pos += BLOCK_SIZE;
            idx += 1;
        }
        let total_blocks = blocks.len();
        Self {
            target,
            blocks,
            total_blocks,
            pending: HashMap::new(),
            start_time: Instant::now(),
            is_complete: false,
        }
    }

    /// Poll for the next OSC message to send. Call periodically (~50 ms).
    pub fn poll(&mut self) -> Option<OscMessage> {
        if self.is_complete {
            return None;
        }
        if self.start_time.elapsed() > TIMEOUT {
            log::error!("Show file transfer to {} timed out", self.target);
            self.is_complete = true;
            return None;
        }

        let target = self.target.clone();
        let total = self.total_blocks;

        // Send any unsent blocks first
        for block in &mut self.blocks {
            if !block.acknowledged && block.sent_at.is_none() {
                block.sent_at = Some(Instant::now());
                self.pending.insert(block.index, block.clone());
                return Some(make_msg(&target, total, block));
            }
        }

        // Retry stale blocks
        for block in &mut self.blocks {
            if !block.acknowledged {
                if let Some(sent) = block.sent_at {
                    if sent.elapsed() > RETRY_AFTER {
                        block.sent_at = Some(Instant::now());
                        return Some(make_msg(&target, total, block));
                    }
                }
            }
        }

        // Check if all done
        let all_ack = self.blocks.iter().all(|b| b.acknowledged);
        if all_ack && self.blocks.len() == self.total_blocks {
            self.is_complete = true;
        }

        None
    }

    pub fn ack(&mut self, block: i32) {
        if block == -1 {
            self.is_complete = true;
            return;
        }
        let idx = block as usize;
        if let Some(b) = self.blocks.iter_mut().find(|b| b.index == idx) {
            b.acknowledged = true;
        }
        self.pending.remove(&(idx));
    }

    pub fn nack(&mut self, block: i32) {
        let idx = block as usize;
        if let Some(b) = self.blocks.iter_mut().find(|b| b.index == idx) {
            b.sent_at = None; // Force re-send on next poll
        }
        self.pending.remove(&(idx));
    }

    pub fn is_complete(&self) -> bool {
        self.is_complete
    }

    pub fn target(&self) -> &str {
        &self.target
    }

}

fn make_msg(target: &str, total_blocks: usize, block: &Block) -> OscMessage {
    OscMessage {
        addr: "/qplayer/remote/update-show".into(),
        args: vec![
            OscType::String(target.to_string()),
            OscType::Int(block.index as i32),
            OscType::Int(total_blocks as i32),
            OscType::Blob(block.data.clone()),
        ],
    }
}

/// Tracks multiple in-flight show file transfers.
#[derive(Default)]
pub struct TransferQueue {
    transfers: Vec<ShowFileTransfer>,
}

impl TransferQueue {
    pub fn push(&mut self, transfer: ShowFileTransfer) {
        self.transfers.push(transfer);
    }

    pub fn ack(&mut self, target: &str, block: i32) {
        for t in &mut self.transfers {
            if t.target() == target {
                t.ack(block);
            }
        }
    }

    pub fn nack(&mut self, target: &str, block: i32) {
        for t in &mut self.transfers {
            if t.target() == target {
                t.nack(block);
            }
        }
    }

    /// Poll all active transfers and collect messages to send.
    pub fn poll_all(&mut self) -> Vec<OscMessage> {
        let mut msgs = Vec::new();
        self.transfers.retain(|t| !t.is_complete());
        for t in &mut self.transfers {
            while let Some(msg) = t.poll() {
                msgs.push(msg);
            }
        }
        msgs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_show_file_transfer() {
        let data: Vec<u8> = (0..2500).map(|i| (i % 256) as u8).collect();
        let mut tx = ShowFileTransfer::new("node1".into(), data.clone());

        // Should produce 3 blocks on first poll
        let msgs: Vec<_> = std::iter::from_fn(|| tx.poll()).collect();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].args[1], OscType::Int(0));
        assert_eq!(msgs[0].args[2], OscType::Int(3));
        assert_eq!(msgs[1].args[1], OscType::Int(1));
        assert_eq!(msgs[2].args[1], OscType::Int(2));

        // Ack block 0 — no immediate retransmission of 1/2 needed
        tx.ack(0);
        let msgs: Vec<_> = std::iter::from_fn(|| tx.poll()).collect();
        assert_eq!(msgs.len(), 0);

        // Nack block 1 — should force immediate re-send
        tx.nack(1);
        let msgs: Vec<_> = std::iter::from_fn(|| tx.poll()).collect();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].args[1], OscType::Int(1));

        // Ack remaining
        tx.ack(1);
        tx.ack(2);
        let msgs: Vec<_> = std::iter::from_fn(|| tx.poll()).collect();
        assert_eq!(msgs.len(), 0);
        assert!(tx.is_complete());
    }
}
