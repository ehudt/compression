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

/// Configuration for the LZ77 matcher.
#[derive(Debug, Clone)]
pub struct MatchConfig {
    /// Minimum match length to emit (zstd minimum is 3).
    pub min_match: usize,
    /// Maximum match length (zstd maximum is 131_074).
    pub max_match: usize,
    /// How many hash-chain links to follow (search depth).
    pub search_depth: usize,
    /// Log2 of the hash table size.
    pub hash_log: usize,
}

impl Default for MatchConfig {
    fn default() -> Self {
        Self {
            min_match: 3,
            max_match: 131_074,
            search_depth: 32,
            hash_log: 17,
        }
    }
}

impl MatchConfig {
    /// Create a config tuned for a given compression level (1-22).
    pub fn for_level(level: i32) -> Self {
        let level = level.clamp(1, 22);
        let search_depth = match level {
            1..=3 => 8,
            4..=7 => 32,
            8..=12 => 128,
            _ => 512,
        };
        let hash_log = match level {
            1..=3 => 14,
            4..=7 => 17,
            _ => 20,
        };
        Self {
            min_match: 3,
            max_match: 131_074,
            search_depth,
            hash_log,
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
        for _ in 0..self.cfg.search_depth {
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
    u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap())
}

#[inline]
fn match_length(data: &[u8], cand_pos: usize, pos: usize, max_len: usize) -> usize {
    let mut len = 4;

    while len + 8 <= max_len
        && data[cand_pos + len..cand_pos + len + 8] == data[pos + len..pos + len + 8]
    {
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

/// Run LZ77 on `data` and produce a sequence of events.
pub fn parse(data: &[u8], cfg: &MatchConfig) -> Vec<Event> {
    let mut finder = MatchFinder::new(cfg.clone());
    let mut events = Vec::new();
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
                    events.push(Event::Literals(lit_start, pos));
                }
                events.push(Event::Match {
                    pos,
                    offset: m.offset,
                    length: m.length,
                });
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
        events.push(Event::Literals(lit_start, data.len()));
    }

    events
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
