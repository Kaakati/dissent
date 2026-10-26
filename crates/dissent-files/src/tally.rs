//! Per-provider accounting for one download. Each provider attempt adds the ciphertext bytes it
//! delivered, and each whole piece those bytes complete is credited to the provider that completed
//! it. A piece is [`PIECE_BYTES`] of ciphertext, the unit the blobs layer checks as it arrives.

/// Ciphertext bytes per verified piece: 64 KiB.
pub const PIECE_BYTES: u64 = 64 * 1024;

/// What one provider contributed to a download so far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceCount {
    /// Position of the provider in the list the download was given.
    pub index: usize,
    /// Ciphertext bytes this provider delivered that are now in the store.
    pub bytes: u64,
    /// Pieces this provider's bytes completed.
    pub pieces: u64,
}

/// The number of pieces in a ciphertext of `cipher_size` bytes. The last piece may be short.
pub fn pieces_of(cipher_size: u64) -> u64 {
    cipher_size.div_ceil(PIECE_BYTES)
}

/// Credits progress to the provider attempt that is running, and reports the counts after each change.
pub(crate) struct Observer<'a> {
    counts: Vec<SourceCount>,
    cipher: u64,
    /// Pieces completed so far, across every provider.
    full: u64,
    /// The provider attempt that is running.
    attempt: usize,
    /// Bytes already in the store when the running attempt began.
    attempt_start: u64,
    sink: &'a mut (dyn FnMut(&[SourceCount]) + Send),
}

impl<'a> Observer<'a> {
    /// An observer for a download of `cipher` bytes from `providers` providers.
    pub(crate) fn new(
        cipher: u64,
        providers: usize,
        sink: &'a mut (dyn FnMut(&[SourceCount]) + Send),
    ) -> Self {
        Observer {
            counts: (0..providers)
                .map(|index| SourceCount {
                    index,
                    bytes: 0,
                    pieces: 0,
                })
                .collect(),
            cipher,
            full: 0,
            attempt: 0,
            attempt_start: 0,
            sink,
        }
    }

    /// Starts the attempt on provider `index`, when `held` bytes are already in the store.
    pub(crate) fn begin(&mut self, index: usize, held: u64) {
        self.attempt = index;
        self.attempt_start = held;
    }

    /// Records that `held` bytes are now in the store, all of them from the running attempt or
    /// from the attempts before it.
    pub(crate) fn observe(&mut self, held: u64) {
        let pieces = pieces_of(self.cipher);
        let full = if held >= self.cipher {
            pieces
        } else {
            held / PIECE_BYTES
        };
        if let Some(count) = self.counts.get_mut(self.attempt) {
            count.bytes = held.saturating_sub(self.attempt_start).min(self.cipher);
            count.pieces += full.saturating_sub(self.full);
        }
        self.full = self.full.max(full);
        (self.sink)(&self.counts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(cipher: u64, steps: &[(usize, u64, u64)]) -> Vec<SourceCount> {
        let mut last = Vec::new();
        {
            let mut sink = |c: &[SourceCount]| last = c.to_vec();
            let mut obs = Observer::new(cipher, 2, &mut sink);
            for &(index, start, held) in steps {
                obs.begin(index, start);
                obs.observe(held);
            }
        }
        last
    }

    #[test]
    fn pieces_round_up_and_the_last_one_may_be_short() {
        assert_eq!(pieces_of(0), 0);
        assert_eq!(pieces_of(1), 1);
        assert_eq!(pieces_of(PIECE_BYTES), 1);
        assert_eq!(pieces_of(PIECE_BYTES + 1), 2);
    }

    #[test]
    fn each_provider_is_credited_with_its_own_bytes_and_pieces() {
        let cipher = 4 * PIECE_BYTES;
        // Provider 0 delivers two pieces, then provider 1 starts from there and finishes.
        let counts = run(
            cipher,
            &[(0, 0, 2 * PIECE_BYTES), (1, 2 * PIECE_BYTES, cipher)],
        );
        assert_eq!(
            counts[0],
            SourceCount {
                index: 0,
                bytes: 2 * PIECE_BYTES,
                pieces: 2
            }
        );
        assert_eq!(
            counts[1],
            SourceCount {
                index: 1,
                bytes: 2 * PIECE_BYTES,
                pieces: 2
            }
        );
        assert_eq!(counts.iter().map(|c| c.bytes).sum::<u64>(), cipher);
        assert_eq!(
            counts.iter().map(|c| c.pieces).sum::<u64>(),
            pieces_of(cipher)
        );
    }

    #[test]
    fn a_partial_piece_is_not_counted_until_it_completes() {
        let counts = run(3 * PIECE_BYTES, &[(0, 0, PIECE_BYTES + 10)]);
        assert_eq!(counts[0].bytes, PIECE_BYTES + 10);
        assert_eq!(counts[0].pieces, 1);
    }
}
