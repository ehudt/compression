//! LZ77 match-finding for zstd compression.
//!
//! Implements a hash-chain based match finder.  The hash table maps 4-byte
//! fingerprints to the most recent position where that fingerprint was seen.
//! Chains allow looking back further for potentially better matches.

/// A parsed LZ77 match.
#[derive(Debug, Clone, Copy)]
pub struct Match {
    /// Distance from the current position to the start of the match.
    pub offset: usize,
    /// Number of bytes matched.
    pub length: usize,
}

/// The compression algorithm family (mirrors zstd's strategy enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Fast,
    DFast,
    Greedy,
    Lazy,
    Lazy2,
    BtLazy2,
    BtOpt,
    BtUltra,
    BtUltra2,
}

/// Configuration for the LZ77 matcher, mirroring zstd's per-level parameters.
#[derive(Debug, Clone)]
pub struct MatchConfig {
    /// Log2 of the history window size (controls how far back matches can reach).
    pub window_log: usize,
    /// Log2 of the hash-chain / binary-tree table size.
    pub chain_log: usize,
    /// Log2 of the hash table size.
    pub hash_log: usize,
    /// Log2 of the maximum search attempts per position.
    pub search_log: usize,
    /// Minimum match length to emit.
    pub min_match: usize,
    /// Early-accept threshold: matches longer than this skip lazy evaluation (0 = disabled).
    pub target_length: usize,
    /// Algorithm family.
    pub strategy: Strategy,
    /// Maximum match length (zstd maximum is 131_074).
    pub max_match: usize,
}

impl MatchConfig {
    /// Derived: actual search attempt count = 1 << search_log.
    #[inline]
    pub fn search_depth(&self) -> usize {
        1 << self.search_log
    }
}

impl Default for MatchConfig {
    /// Returns level-5 (Greedy) parameters — the first level using the
    /// greedy algorithm that all current code implements.
    fn default() -> Self {
        Self::for_level(5)
    }
}

impl MatchConfig {
    /// Create a config tuned for a given compression level (1-22).
    ///
    /// Parameters match the reference zstd implementation's `clevels.h` table
    /// for inputs > 256 KiB. Levels 20-22 use level-19 values.
    pub fn for_level(level: i32) -> Self {
        let level = level.clamp(1, 22);
        let (window_log, chain_log, hash_log, search_log, min_match, target_length, strategy) =
            match level {
                1 => (19, 13, 14, 1, 7, 0, Strategy::Fast),
                2 => (20, 15, 16, 1, 6, 0, Strategy::Fast),
                3 => (21, 16, 17, 1, 5, 0, Strategy::DFast),
                4 => (21, 18, 18, 1, 5, 0, Strategy::DFast),
                5 => (21, 18, 19, 3, 5, 2, Strategy::Greedy),
                6 => (21, 18, 19, 3, 5, 4, Strategy::Lazy),
                7 => (21, 19, 20, 4, 5, 8, Strategy::Lazy),
                8 => (21, 19, 20, 4, 5, 16, Strategy::Lazy2),
                9 => (22, 20, 21, 4, 5, 16, Strategy::Lazy2),
                10 => (22, 21, 22, 5, 5, 16, Strategy::Lazy2),
                11 => (22, 21, 22, 6, 5, 16, Strategy::Lazy2),
                12 => (22, 22, 23, 6, 5, 32, Strategy::Lazy2),
                13 => (22, 22, 22, 4, 5, 32, Strategy::BtLazy2),
                14 => (22, 22, 23, 5, 5, 32, Strategy::BtLazy2),
                15 => (22, 23, 23, 6, 5, 32, Strategy::BtLazy2),
                16 => (22, 22, 22, 5, 5, 48, Strategy::BtOpt),
                17 => (23, 23, 22, 5, 4, 64, Strategy::BtOpt),
                18 => (23, 23, 22, 6, 3, 64, Strategy::BtUltra),
                // Levels 19-22 all use level-19 values.
                _ => (23, 24, 22, 7, 3, 256, Strategy::BtUltra2),
            };
        Self {
            window_log,
            chain_log,
            hash_log,
            search_log,
            min_match,
            target_length,
            strategy,
            max_match: 131_074,
        }
    }
}

/// An LZ77 match finder with a hash table and chain links.
pub struct MatchFinder {
    cfg: MatchConfig,
    /// Hash table: position of the last occurrence of a 4-byte fingerprint.
    hash_table: Vec<u32>,
    /// Chain: `chain[pos & window_mask]` = previous position with the same hash.
    chain: Vec<u32>,
    window_mask: usize,
}

const WINDOW_LOG: usize = 17; // 128 KiB window (zstd default for level 1-3)
const WINDOW_SIZE: usize = 1 << WINDOW_LOG;

const HASH_PRIME: u64 = 0x9E3779B1_9E3779B1;

impl MatchFinder {
    /// Create a new `MatchFinder` with the given configuration.
    pub fn new(cfg: MatchConfig) -> Self {
        let table_size = 1usize << cfg.hash_log;
        let window_mask = WINDOW_SIZE - 1;
        Self {
            cfg,
            hash_table: vec![u32::MAX; table_size],
            chain: vec![u32::MAX; WINDOW_SIZE],
            window_mask,
        }
    }

    /// Hash a 4-byte sequence starting at `pos` in `data`.
    fn hash4(&self, data: &[u8], pos: usize) -> usize {
        let bytes = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as u64;
        let h = bytes.wrapping_mul(HASH_PRIME);
        (h >> (64 - self.cfg.hash_log)) as usize
    }

    /// Insert position `pos` into the hash table and return the previous entry.
    fn insert(&mut self, data: &[u8], pos: usize) -> u32 {
        let h = self.hash4(data, pos);
        let prev = self.hash_table[h];
        self.hash_table[h] = pos as u32;
        self.chain[pos & self.window_mask] = prev;
        prev
    }

    /// Find the best match starting at `pos` in `data`.
    pub fn find_match(&mut self, data: &[u8], pos: usize) -> Option<Match> {
        if pos + 4 > data.len() {
            return None;
        }

        let mut candidate = self.insert(data, pos);
        let mut best_len = 0usize;
        let mut best_off = 0usize;
        let max_offset = WINDOW_SIZE;
        let needle = load_u32(data, pos);
        let max_len = (data.len() - pos).min(self.cfg.max_match);
        for _ in 0..self.cfg.search_depth() {
            if candidate == u32::MAX {
                break;
            }
            let cand_pos = candidate as usize;
            if cand_pos >= pos || pos - cand_pos > max_offset {
                break;
            }
            let offset = pos - cand_pos;

            // Check first 4 bytes quickly
            if load_u32(data, cand_pos) != needle {
                candidate = self.chain[cand_pos & self.window_mask];
                continue;
            }

            // Extend match
            let len = match_length(data, cand_pos, pos, max_len);

            if len > best_len {
                best_len = len;
                best_off = offset;
                if len >= self.cfg.max_match {
                    break;
                }
            }

            candidate = self.chain[cand_pos & self.window_mask];
        }

        if best_len >= self.cfg.min_match {
            Some(Match {
                offset: best_off,
                length: best_len,
            })
        } else {
            None
        }
    }

    /// Register all positions in a literal run (so the hash table stays up-to-date).
    pub fn skip(&mut self, data: &[u8], pos: usize, length: usize) {
        let sparse_step = match length {
            0..=24 => 1,
            25..=96 => 2,
            97..=256 => 4,
            _ => 8,
        };
        let dense_prefix = 8usize;
        let dense_suffix = 8usize;
        let Some(valid_len) = data.len().checked_sub(pos + 4).map(|tail| tail + 1) else {
            return;
        };
        let limit = length.min(valid_len);
        let prefix_end = dense_prefix.min(limit);

        for i in 0..prefix_end {
            self.insert(data, pos + i);
        }

        let middle_end = length.saturating_sub(dense_suffix).min(limit);
        if prefix_end < middle_end {
            let middle_start = prefix_end.next_multiple_of(sparse_step);
            for i in (middle_start..middle_end).step_by(sparse_step) {
                self.insert(data, pos + i);
            }
        }

        for i in middle_end.max(prefix_end)..limit {
            self.insert(data, pos + i);
        }
    }
}

#[inline]
fn load_u32(data: &[u8], pos: usize) -> u32 {
    debug_assert!(pos + 4 <= data.len());
    unsafe { u32::from_le(data.as_ptr().add(pos).cast::<u32>().read_unaligned()) }
}

#[inline]
fn load_u64(data: &[u8], pos: usize) -> u64 {
    debug_assert!(pos + 8 <= data.len());
    unsafe { u64::from_le(data.as_ptr().add(pos).cast::<u64>().read_unaligned()) }
}

#[inline]
fn match_length(data: &[u8], cand_pos: usize, pos: usize, max_len: usize) -> usize {
    let mut len = 4;

    while len + 8 <= max_len && load_u64(data, cand_pos + len) == load_u64(data, pos + len) {
        len += 8;
    }

    while len < max_len && data[cand_pos + len] == data[pos + len] {
        len += 1;
    }

    len
}

/// A parsed "event" produced by the LZ77 pass.
#[derive(Debug, Clone)]
pub enum Event {
    /// Literal bytes that should be emitted as-is.
    Literals(usize, usize), // (start, end)
    /// A back-reference match.
    Match {
        pos: usize,
        offset: usize,
        length: usize,
    },
}

pub trait ParseSink {
    fn literals(&mut self, start: usize, end: usize);
    fn matched(&mut self, pos: usize, offset: usize, length: usize);
}

impl<T: ParseSink + ?Sized> ParseSink for &mut T {
    fn literals(&mut self, start: usize, end: usize) {
        (**self).literals(start, end);
    }

    fn matched(&mut self, pos: usize, offset: usize, length: usize) {
        (**self).matched(pos, offset, length);
    }
}

struct EventSink<'a> {
    events: &'a mut Vec<Event>,
}

impl ParseSink for EventSink<'_> {
    fn literals(&mut self, start: usize, end: usize) {
        self.events.push(Event::Literals(start, end));
    }

    fn matched(&mut self, pos: usize, offset: usize, length: usize) {
        self.events.push(Event::Match {
            pos,
            offset,
            length,
        });
    }
}

/// Run LZ77 on `data` and produce a sequence of events.
pub fn parse(data: &[u8], cfg: &MatchConfig) -> Vec<Event> {
    let mut events = Vec::new();
    parse_with_sink(
        data,
        cfg,
        EventSink {
            events: &mut events,
        },
    );
    events
}

/// Run LZ77 on `data` and stream events to `sink`.
pub fn parse_with_sink(data: &[u8], cfg: &MatchConfig, sink: impl ParseSink) {
    parse_ranges(data, cfg, sink);
}

/// Run LZ77 on `data`, streaming literal ranges and matches through `sink`.
pub fn parse_ranges(data: &[u8], cfg: &MatchConfig, mut sink: impl ParseSink) {
    let mut finder = MatchFinder::new(cfg.clone());
    let mut pos = 0;
    let mut lit_start = 0;

    while pos < data.len() {
        if pos + 4 > data.len() {
            // Not enough bytes left for a match; emit as literals.
            break;
        }

        match finder.find_match(data, pos) {
            Some(m) if m.length >= cfg.min_match => {
                if pos > lit_start {
                    sink.literals(lit_start, pos);
                }
                sink.matched(pos, m.offset, m.length);
                // Skip matched positions (already inserted current pos)
                finder.skip(data, pos + 1, m.length - 1);
                pos += m.length;
                lit_start = pos;
            }
            _ => {
                pos += 1;
            }
        }
    }

    if lit_start < data.len() {
        sink.literals(lit_start, data.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_match_on_random_data() {
        let data: Vec<u8> = (0..128).map(|i| (i * 37 + 11) as u8).collect();
        let events = parse(&data, &MatchConfig::default());
        // Should mostly be literals
        let literal_bytes: usize = events
            .iter()
            .filter_map(|e| {
                if let Event::Literals(s, end) = e {
                    Some(end - s)
                } else {
                    None
                }
            })
            .sum();
        assert!(literal_bytes > 0);
    }

    #[test]
    fn test_match_on_repetitive_data() {
        let data = b"abcabcabcabcabcabcabcabc".repeat(4);
        let events = parse(&data, &MatchConfig::default());
        let has_match = events.iter().any(|e| matches!(e, Event::Match { .. }));
        assert!(has_match, "should find matches in repetitive data");
    }

    /// Expected params for each level: (window_log, chain_log, hash_log, search_log, min_match, target_length)
    #[rustfmt::skip]
    const LEVEL_PARAMS: &[(usize, usize, usize, usize, usize, usize)] = &[
        (19, 13, 14, 1, 7,   0),  // 1
        (20, 15, 16, 1, 6,   0),  // 2
        (21, 16, 17, 1, 5,   0),  // 3
        (21, 18, 18, 1, 5,   0),  // 4
        (21, 18, 19, 3, 5,   2),  // 5
        (21, 18, 19, 3, 5,   4),  // 6
        (21, 19, 20, 4, 5,   8),  // 7
        (21, 19, 20, 4, 5,  16),  // 8
        (22, 20, 21, 4, 5,  16),  // 9
        (22, 21, 22, 5, 5,  16),  // 10
        (22, 21, 22, 6, 5,  16),  // 11
        (22, 22, 23, 6, 5,  32),  // 12
        (22, 22, 22, 4, 5,  32),  // 13
        (22, 22, 23, 5, 5,  32),  // 14
        (22, 23, 23, 6, 5,  32),  // 15
        (22, 22, 22, 5, 5,  48),  // 16
        (23, 23, 22, 5, 4,  64),  // 17
        (23, 23, 22, 6, 3,  64),  // 18
        (23, 24, 22, 7, 3, 256),  // 19
    ];

    #[rustfmt::skip]
    const LEVEL_STRATEGIES: &[Strategy] = &[
        Strategy::Fast,     // 1
        Strategy::Fast,     // 2
        Strategy::DFast,    // 3
        Strategy::DFast,    // 4
        Strategy::Greedy,   // 5
        Strategy::Lazy,     // 6
        Strategy::Lazy,     // 7
        Strategy::Lazy2,    // 8
        Strategy::Lazy2,    // 9
        Strategy::Lazy2,    // 10
        Strategy::Lazy2,    // 11
        Strategy::Lazy2,    // 12
        Strategy::BtLazy2,  // 13
        Strategy::BtLazy2,  // 14
        Strategy::BtLazy2,  // 15
        Strategy::BtOpt,    // 16
        Strategy::BtOpt,    // 17
        Strategy::BtUltra,  // 18
        Strategy::BtUltra2, // 19
    ];

    #[test]
    fn test_for_level_params() {
        for (i, &(wlog, clog, hlog, slog, mml, tlen)) in LEVEL_PARAMS.iter().enumerate() {
            let level = (i + 1) as i32;
            let cfg = MatchConfig::for_level(level);
            assert_eq!(cfg.window_log, wlog, "window_log mismatch at level {level}");
            assert_eq!(cfg.chain_log, clog, "chain_log mismatch at level {level}");
            assert_eq!(cfg.hash_log, hlog, "hash_log mismatch at level {level}");
            assert_eq!(cfg.search_log, slog, "search_log mismatch at level {level}");
            assert_eq!(cfg.min_match, mml, "min_match mismatch at level {level}");
            assert_eq!(cfg.target_length, tlen, "target_length mismatch at level {level}");
        }
    }

    #[test]
    fn test_for_level_strategy() {
        for (i, &expected) in LEVEL_STRATEGIES.iter().enumerate() {
            let level = (i + 1) as i32;
            let cfg = MatchConfig::for_level(level);
            assert_eq!(cfg.strategy, expected, "strategy mismatch at level {level}");
        }
    }

    #[test]
    fn test_for_level_clamp() {
        // Levels 20-22 should return the same params as level 19.
        let base = MatchConfig::for_level(19);
        for level in [20, 21, 22] {
            let cfg = MatchConfig::for_level(level);
            assert_eq!(cfg.window_log, base.window_log, "level {level}");
            assert_eq!(cfg.chain_log, base.chain_log, "level {level}");
            assert_eq!(cfg.hash_log, base.hash_log, "level {level}");
            assert_eq!(cfg.search_log, base.search_log, "level {level}");
            assert_eq!(cfg.min_match, base.min_match, "level {level}");
            assert_eq!(cfg.target_length, base.target_length, "level {level}");
            assert_eq!(cfg.strategy, base.strategy, "level {level}");
        }
    }

    #[test]
    fn test_reconstructs_original() {
        let original = b"hello world, hello world again, hello everyone!";
        let events = parse(original, &MatchConfig::default());

        let mut reconstructed = Vec::new();
        let mut pos = 0usize;
        for event in &events {
            match event {
                Event::Literals(s, e) => {
                    reconstructed.extend_from_slice(&original[*s..*e]);
                }
                Event::Match {
                    pos: _,
                    offset,
                    length,
                } => {
                    let src_start = reconstructed.len() - offset;
                    for i in 0..*length {
                        let b = reconstructed[src_start + i];
                        reconstructed.push(b);
                    }
                    pos += length;
                }
            }
        }
        assert_eq!(reconstructed, original);
    }
}
