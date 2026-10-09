//! Boîte aux lettres à une place entre capture et vision (§3.2, §7.1).
//!
//! Politique « dernière trame gagne » : un triple tampon échange des indices,
//! jamais des copies, sans verrou. La trame non lue est écrasée par la suivante
//! et comptée comme abandonnée : la latence ne s'accumule jamais.

use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, bounded};

use crate::Frame;

const DIRTY: u8 = 0b100;
const INDEX: u8 = 0b011;

struct Shared {
    buffers: [UnsafeCell<Frame>; 3],
    /// Indice du tampon intermédiaire, plus le bit `DIRTY` s'il contient une trame non lue.
    middle: AtomicU8,
    dropped: AtomicU64,
}

// SAFETY: chaque tampon n'est accessible qu'à un seul côté à la fois. Le
// producteur possède l'indice `back`, le consommateur l'indice `front`, et le
// troisième indice n'est échangé que par `AtomicU8::swap`.
unsafe impl Sync for Shared {}
unsafe impl Send for Shared {}

/// Côté capture.
pub struct Producer {
    shared: Arc<Shared>,
    back: u8,
    wake: Sender<()>,
}

/// Côté vision.
pub struct Consumer {
    shared: Arc<Shared>,
    front: u8,
    wake: Receiver<()>,
}

/// Crée une boîte aux lettres vide.
pub fn mailbox() -> (Producer, Consumer) {
    let shared = Arc::new(Shared {
        buffers: [
            UnsafeCell::new(Frame::default()),
            UnsafeCell::new(Frame::default()),
            UnsafeCell::new(Frame::default()),
        ],
        middle: AtomicU8::new(1),
        dropped: AtomicU64::new(0),
    });
    let (wake_tx, wake_rx) = bounded(1);
    (
        Producer {
            shared: shared.clone(),
            back: 0,
            wake: wake_tx,
        },
        Consumer {
            shared,
            front: 2,
            wake: wake_rx,
        },
    )
}

impl Producer {
    /// Tampon dans lequel écrire la prochaine trame (réutilisé, jamais réalloué
    /// une fois à la bonne taille).
    pub fn buffer(&mut self) -> &mut Frame {
        // SAFETY: `back` n'appartient qu'au producteur.
        unsafe { &mut *self.shared.buffers[usize::from(self.back)].get() }
    }

    /// Publie le tampon courant. Si la trame précédente n'a pas été lue, elle
    /// est abandonnée.
    pub fn publish(&mut self) {
        let old = self.shared.middle.swap(self.back | DIRTY, Ordering::AcqRel);
        if old & DIRTY != 0 {
            self.shared.dropped.fetch_add(1, Ordering::Relaxed);
        }
        self.back = old & INDEX;
        // Réveil non bloquant : un jeton suffit.
        let _ = self.wake.try_send(());
    }

    pub fn dropped(&self) -> u64 {
        self.shared.dropped.load(Ordering::Relaxed)
    }
}

impl Consumer {
    fn take(&mut self) -> bool {
        if self.shared.middle.load(Ordering::Acquire) & DIRTY == 0 {
            return false;
        }
        let old = self.shared.middle.swap(self.front, Ordering::AcqRel);
        self.front = old & INDEX;
        true
    }

    /// Attend la trame la plus récente, au plus `timeout`.
    pub fn recv_timeout(&mut self, timeout: Duration) -> Option<&Frame> {
        if !self.take() {
            let _ = self.wake.recv_timeout(timeout);
            if !self.take() {
                return None;
            }
        }
        // Le jeton éventuel correspond à la trame que l'on vient de prendre.
        let _ = self.wake.try_recv();
        // SAFETY: `front` n'appartient qu'au consommateur.
        Some(unsafe { &*self.shared.buffers[usize::from(self.front)].get() })
    }

    /// Nombre de trames écrasées avant d'avoir été lues.
    pub fn dropped(&self) -> u64 {
        self.shared.dropped.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn publish(p: &mut Producer, seq: u64) {
        let f = p.buffer();
        f.seq = seq;
        f.data.clear();
        f.data.extend_from_slice(&seq.to_le_bytes());
        p.publish();
    }

    #[test]
    fn empty_mailbox_times_out() {
        let (_p, mut c) = mailbox();
        assert!(c.recv_timeout(Duration::from_millis(5)).is_none());
    }

    #[test]
    fn latest_frame_wins_and_drops_are_counted() {
        let (mut p, mut c) = mailbox();
        publish(&mut p, 1);
        publish(&mut p, 2);
        publish(&mut p, 3);
        assert_eq!(c.recv_timeout(Duration::from_millis(5)).unwrap().seq, 3);
        assert_eq!(c.dropped(), 2);
        assert!(c.recv_timeout(Duration::from_millis(5)).is_none());
        publish(&mut p, 4);
        assert_eq!(c.recv_timeout(Duration::from_millis(5)).unwrap().seq, 4);
        assert_eq!(c.dropped(), 2);
    }

    #[test]
    fn frames_stay_consistent_across_threads() {
        let (mut p, mut c) = mailbox();
        const N: u64 = 200_000;
        let producer = std::thread::spawn(move || {
            for seq in 1..=N {
                publish(&mut p, seq);
            }
        });
        let mut last = 0;
        let mut received = 0u64;
        while last < N {
            if let Some(f) = c.recv_timeout(Duration::from_millis(200)) {
                assert!(f.seq > last, "séquence non monotone");
                assert_eq!(f.data, f.seq.to_le_bytes(), "trame déchirée");
                last = f.seq;
                received += 1;
            }
        }
        producer.join().unwrap();
        assert_eq!(received + c.dropped(), N);
    }
}
