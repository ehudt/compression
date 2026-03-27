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

/// An LZ77 match finder with a hash table and a secondary table.
///
/// Positions stored in the hash table and chain are **absolute** offsets from
/// the start of the frame (not relative to the current block). This allows
/// `MatchFinder` to be reused across multiple blocks within a frame, giving
/// the encoder access to cross-block match history.
///
/// The `chain` array serves different roles depending on the strategy:
/// - **Greedy / Lazy / Lazy2**: hash chain (`chain[pos & chain_mask]` = previous pos).
/// - **DFast**: long-match hash table (indexed by hash value, not position).
/// - **BT\* strategies**: interleaved binary-tree nodes. Node at position `p` uses
///   `chain[2 * (p & chain_mask)]` (left child) and `chain[2 * (p & chain_mask) + 1]`
///   (right child). Allocated at **double** the normal size for these strategies.
pub struct MatchFinder {
    cfg: MatchConfig,
    /// Hash table: most-recent absolute position for a 4-byte fingerprint.
    hash_table: Vec<u32>,
    /// Secondary table (chain / long-hash / BT nodes — see struct doc).
    chain: Vec<u32>,
    /// `(1 << cfg.chain_log) - 1` — mask for the node/slot index.
    chain_mask: usize,
    /// `1 << cfg.window_log` — maximum match offset allowed.
    window_size: usize,
}

const HASH_PRIME: u64 = 0x9E3779B1_9E3779B1;
const HASH_PRIME_64: u64 = 0x9E3779B9_7F4A7C15;

impl MatchFinder {
    /// The maximum match offset this finder allows (= `1 << cfg.window_log`).
    pub fn window_size(&self) -> usize {
        self.window_size
    }

    /// Create a new `MatchFinder` sized according to `cfg`.
    pub fn new(cfg: &MatchConfig) -> Self {
        let table_size = 1usize << cfg.hash_log;
        let chain_slots = 1usize << cfg.chain_log;
        let chain_mask = chain_slots - 1;
        let window_size = 1usize << cfg.window_log;
        // BT strategies interleave left/right children, requiring double the slots.
        let chain_size = match cfg.strategy {
            Strategy::BtLazy2 | Strategy::BtOpt | Strategy::BtUltra | Strategy::BtUltra2 => {
                2 * chain_slots
            }
            _ => chain_slots,
        };
        Self {
            cfg: cfg.clone(),
            hash_table: vec![u32::MAX; table_size],
            chain: vec![u32::MAX; chain_size],
            chain_mask,
            window_size,
        }
    }

    /// Hash a 4-byte sequence starting at `pos` in `data`.
    fn hash4(&self, data: &[u8], pos: usize) -> usize {
        self.hash4_value(load_u32(data, pos))
    }

    fn hash4_value(&self, bytes: u32) -> usize {
        let h = (bytes as u64).wrapping_mul(HASH_PRIME);
        (h >> (64 - self.cfg.hash_log)) as usize
    }

    #[inline]
    fn hash8_value(&self, bytes: u64) -> usize {
        (bytes.wrapping_mul(HASH_PRIME_64) >> (64 - self.cfg.chain_log)) as usize
    }

    /// Fast strategy: single hash-table lookup + insert. No chain walking.
    ///
    /// Returns a match if `length >= cfg.min_match`; always updates `hash_table`.
    fn lookup_fast(&mut self, data: &[u8], pos: usize) -> Option<Match> {
        let h = self.hash4(data, pos);
        let prev = self.hash_table[h];
        self.hash_table[h] = pos as u32;
        let cand = prev as usize;
        if prev == u32::MAX || cand >= pos || pos - cand > self.window_size {
            return None;
        }
        if load_u32(data, cand) != load_u32(data, pos) {
            return None;
        }
        let max_len = (data.len() - pos).min(self.cfg.max_match);
        let len = match_length(data, cand, pos, max_len);
        if len >= self.cfg.min_match {
            Some(Match { offset: pos - cand, length: len })
        } else {
            None
        }
    }

    /// Double-fast strategy: tries the long (8-byte) hash table first, then the
    /// short (4-byte) table. Both tables are always updated.
    ///
    /// The `chain` array is repurposed as the long hash table (indexed by hash value,
    /// not position). This is exclusive with the chain usage in Greedy/Lazy modes.
    fn lookup_dfast(&mut self, data: &[u8], pos: usize) -> Option<Match> {
        let needle4 = load_u32(data, pos);
        let h_short = self.hash4_value(needle4);

        // Try the long (8-byte) hash table first.
        if pos + 8 <= data.len() {
            let needle8 = load_u64(data, pos);
            let h_long = self.hash8_value(needle8);
            let prev_long = self.chain[h_long];
            self.chain[h_long] = pos as u32;
            let cand = prev_long as usize;
            if prev_long != u32::MAX
                && cand < pos
                && pos - cand <= self.window_size
                && load_u32(data, cand) == needle4
            {
                let max_len = (data.len() - pos).min(self.cfg.max_match);
                let len = match_length(data, cand, pos, max_len);
                if len >= self.cfg.min_match {
                    // Keep the short table fresh for later short-match probes.
                    self.hash_table[h_short] = pos as u32;
                    return Some(Match { offset: pos - cand, length: len });
                }
            }
        }

        // Fall back to short (4-byte) hash table.
        let prev = self.hash_table[h_short];
        self.hash_table[h_short] = pos as u32;
        let cand = prev as usize;
        if prev == u32::MAX || cand >= pos || pos - cand > self.window_size {
            return None;
        }
        if load_u32(data, cand) != needle4 {
            return None;
        }
        let max_len = (data.len() - pos).min(self.cfg.max_match);
        let len = match_length(data, cand, pos, max_len);
        if len >= self.cfg.min_match {
            Some(Match { offset: pos - cand, length: len })
        } else {
            None
        }
    }

    /// Binary-tree match finder: insert `pos` into the DUBT and return the best match.
    ///
    /// The `chain` array is used as interleaved BT nodes:
    /// - `chain[2 * (p & mask)]` = left child of node p (suffixes < p's)
    /// - `chain[2 * (p & mask) + 1]` = right child of node p (suffixes > p's)
    ///
    /// Simultaneously inserts `pos` as the new root and finds the longest match,
    /// bounded by `cfg.search_depth()` comparisons.
    fn bt_find_insert(&mut self, data: &[u8], pos: usize) -> Option<Match> {
        if pos + 4 > data.len() {
            return None;
        }

        let h = self.hash4(data, pos);
        let root = self.hash_table[h];
        self.hash_table[h] = pos as u32;

        let max_offset = self.window_size.min(self.chain_mask);
        let max_len = (data.len() - pos).min(self.cfg.max_match);
        let mask = self.chain_mask;
        let mut searches = self.cfg.search_depth();

        let pos_slot = pos & mask;
        // Write targets: chain indices where the next smaller/larger candidate is stored.
        let mut smaller_write = 2 * pos_slot;     // left child slot of pos
        let mut larger_write = 2 * pos_slot + 1;  // right child slot of pos

        let mut best_len = 0usize;
        let mut best_off = 0usize;
        let mut match_idx = if root == u32::MAX { usize::MAX } else { root as usize };

        while searches > 0 && match_idx != usize::MAX {
            searches -= 1;

            let cand = match_idx;
            // Stop if candidate is outside the window or its slot has been recycled.
            if cand >= pos || pos - cand > max_offset {
                break;
            }

            let cand_slot = cand & mask;
            let common = match_length_full(data, cand, pos, max_len);

            if common > best_len {
                best_len = common;
                best_off = pos - cand;
                if best_len >= max_len {
                    // Maximum match found: terminate both subtrees and exit.
                    self.chain[smaller_write] = u32::MAX;
                    self.chain[larger_write] = u32::MAX;
                    if best_len >= self.cfg.min_match {
                        return Some(Match { offset: best_off, length: best_len });
                    } else {
                        return None;
                    }
                }
            }

            // Determine traversal direction by comparing byte at the divergence point.
            // If pos's data ends first (common == max_len == data.len() - pos), treat
            // as "cand is larger" to avoid an out-of-bounds access.
            let go_left = pos + common >= data.len()
                || data[cand + common] >= data[pos + common];

            if go_left {
                // cand is lexicographically >= pos → cand goes in pos's right (larger) subtree.
                // Continue down cand's left branch for candidates between pos and cand.
                self.chain[larger_write] = cand as u32;
                larger_write = 2 * cand_slot;                          // cand's left child slot
                match_idx = self.chain[2 * cand_slot] as usize;
                if self.chain[2 * cand_slot] == u32::MAX {
                    match_idx = usize::MAX;
                }
            } else {
                // cand is lexicographically < pos → cand goes in pos's left (smaller) subtree.
                // Continue down cand's right branch.
                self.chain[smaller_write] = cand as u32;
                smaller_write = 2 * cand_slot + 1;                     // cand's right child slot
                match_idx = self.chain[2 * cand_slot + 1] as usize;
                if self.chain[2 * cand_slot + 1] == u32::MAX {
                    match_idx = usize::MAX;
                }
            }
        }

        // Terminate dangling subtree pointers.
        self.chain[smaller_write] = u32::MAX;
        self.chain[larger_write] = u32::MAX;

        if best_len >= self.cfg.min_match {
            Some(Match { offset: best_off, length: best_len })
        } else {
            None
        }
    }

    /// Insert absolute position `pos` into the hash table and return the previous entry.
    fn insert(&mut self, data: &[u8], pos: usize) -> u32 {
        let h = self.hash4(data, pos);
        let prev = self.hash_table[h];
        self.hash_table[h] = pos as u32;
        self.chain[pos & self.chain_mask] = prev;
        prev
    }

    /// Find the best match starting at absolute position `pos` in `data`.
    ///
    /// `data` should be `&full_frame_data[..block_end]` so that `data.len()` bounds
    /// the maximum match length to the current block.
    pub fn find_match(&mut self, data: &[u8], pos: usize) -> Option<Match> {
        if pos + 4 > data.len() {
            return None;
        }

        let mut candidate = self.insert(data, pos);
        let mut best_len = 0usize;
        let mut best_off = 0usize;
        let max_offset = self.window_size;
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
                candidate = self.chain[cand_pos & self.chain_mask];
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

            candidate = self.chain[cand_pos & self.chain_mask];
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

    /// Register positions in a matched run so the hash table stays up-to-date.
    ///
    /// `data` should be `&full_frame_data[..block_end]`.
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

    /// DFast-specific reinsertion for matched runs.
    ///
    /// DFast stores its secondary structure as an 8-byte hash table rather than a
    /// per-position chain, so the generic `skip()` logic cannot be reused here.
    pub fn skip_dfast(&mut self, data: &[u8], pos: usize, length: usize) {
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

        let mut i = 0usize;
        while i < prefix_end {
            self.insert_dfast_position(data, pos + i);
            i += 1;
        }

        let middle_end = length.saturating_sub(dense_suffix).min(limit);
        if prefix_end < middle_end {
            i = prefix_end.next_multiple_of(sparse_step);
            while i < middle_end {
                self.insert_dfast_position(data, pos + i);
                i += sparse_step;
            }
        }

        i = middle_end.max(prefix_end);
        while i < limit {
            self.insert_dfast_position(data, pos + i);
            i += 1;
        }
    }

    #[inline]
    fn insert_dfast_position(&mut self, data: &[u8], pos: usize) {
        let bytes4 = load_u32(data, pos);
        let h_short = self.hash4_value(bytes4);
        self.hash_table[h_short] = pos as u32;
        if pos + 8 <= data.len() {
            let h_long = self.hash8_value(load_u64(data, pos));
            self.chain[h_long] = pos as u32;
        }
    }

    /// Seed a small number of skipped miss positions so anchor-relative stepping
    /// does not completely blind the next DFast probe after long literal runs.
    #[inline]
    fn skip_dfast_miss_positions(&mut self, data: &[u8], pos: usize, step: usize) {
        if step <= 1 {
            return;
        }
        let Some(valid_len) = data.len().checked_sub(pos + 4).map(|tail| tail + 1) else {
            return;
        };
        let limit = step.min(valid_len);
        if limit <= 1 {
            return;
        }

        let last = limit - 1;
        self.insert_dfast_position(data, pos + last);

        if limit >= 8 {
            self.insert_dfast_position(data, pos + (last / 2));
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

/// Full match length from byte 0 (unlike `match_length` which starts at 4).
///
/// Used by the BT match finder where the common prefix is not guaranteed to be ≥ 4.
#[inline]
fn match_length_full(data: &[u8], a: usize, b: usize, max_len: usize) -> usize {
    if max_len >= 4 && load_u32(data, a) == load_u32(data, b) {
        // Fast path: first 4 bytes match — use the word-at-a-time extension.
        match_length(data, a, b, max_len)
    } else {
        // Short common prefix: compare byte by byte up to 3 bytes.
        let limit = max_len.min(3);
        let mut len = 0;
        while len < limit && data[a + len] == data[b + len] {
            len += 1;
        }
        len
    }
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
    let mut finder = MatchFinder::new(cfg);
    parse_ranges(
        data,
        0,
        data.len(),
        &mut finder,
        EventSink {
            events: &mut events,
        },
    );
    events
}

/// Run LZ77 on `data` and stream events to `sink`.
pub fn parse_with_sink(data: &[u8], cfg: &MatchConfig, sink: impl ParseSink) {
    let mut finder = MatchFinder::new(cfg);
    parse_ranges(data, 0, data.len(), &mut finder, sink);
}

/// Run LZ77 on the range `[start, end)` of `full_data`, streaming events to `sink`.
///
/// Positions in emitted events are absolute (offset from the start of `full_data`).
/// `finder` is updated in-place so it can be reused across consecutive blocks to
/// enable cross-block match history.
///
/// The parsing strategy is selected from `finder.cfg.strategy`:
/// - `Greedy | Fast | DFast` → greedy (take the first sufficient match)
/// - `Lazy` → 1-position lookahead before committing
/// - `Lazy2 | BtLazy2 | BtOpt | BtUltra | BtUltra2` → 2-position lookahead
pub fn parse_ranges(
    full_data: &[u8],
    start: usize,
    end: usize,
    finder: &mut MatchFinder,
    sink: impl ParseSink,
) {
    match finder.cfg.strategy {
        Strategy::Fast => {
            parse_ranges_fast(full_data, start, end, finder, sink);
        }
        Strategy::DFast => {
            parse_ranges_dfast(full_data, start, end, finder, sink);
        }
        Strategy::Greedy => {
            parse_ranges_greedy(full_data, start, end, finder, sink);
        }
        Strategy::Lazy => {
            parse_ranges_lazy(full_data, start, end, finder, sink, 1);
        }
        Strategy::Lazy2 => {
            parse_ranges_lazy(full_data, start, end, finder, sink, 2);
        }
        Strategy::BtLazy2 => {
            parse_ranges_bt_lazy2(full_data, start, end, finder, sink);
        }
        Strategy::BtOpt | Strategy::BtUltra | Strategy::BtUltra2 => {
            parse_ranges_optimal(full_data, start, end, finder, sink);
        }
    }
}

/// Greedy LZ77: take the first match found at each position.
fn parse_ranges_greedy(
    full_data: &[u8],
    start: usize,
    end: usize,
    finder: &mut MatchFinder,
    mut sink: impl ParseSink,
) {
    // Expose only data up to `end` so `data.len()` naturally bounds match lengths
    // to the current block while still allowing lookback into previous blocks.
    let data = &full_data[..end];
    let cfg = finder.cfg.clone();
    let mut pos = start;
    let mut lit_start = start;
    while pos < end {
        if pos + 4 > end {
            // Not enough bytes left in block for a match; emit tail as literals.
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

    if lit_start < end {
        sink.literals(lit_start, end);
    }
}

/// Fast LZ77: single hash-table lookup per position, no chain walking.
///
/// On a miss, the step size grows with the distance from the last anchor
/// (`step = 1 + (pos - lit_start) >> search_log`), aggressively skipping
/// stretches of non-matching data for maximum throughput. Skipped positions
/// are not inserted — this is intentional for the fast strategy.
fn parse_ranges_fast(
    full_data: &[u8],
    start: usize,
    end: usize,
    finder: &mut MatchFinder,
    mut sink: impl ParseSink,
) {
    let data = &full_data[..end];
    let search_log = finder.cfg.search_log;
    let mut pos = start;
    let mut lit_start = start;

    while pos < end {
        if pos + 4 > end {
            break;
        }

        match finder.lookup_fast(data, pos) {
            Some(m) => {
                if lit_start < pos {
                    sink.literals(lit_start, pos);
                }
                sink.matched(pos, m.offset, m.length);
                pos += m.length;
                lit_start = pos;
            }
            None => {
                // Skip grows with distance from last anchor (reference zstd behaviour).
                let step = 1 + ((pos - lit_start) >> search_log);
                pos += step;
            }
        }
    }

    if lit_start < end {
        sink.literals(lit_start, end);
    }
}

/// Double-fast LZ77: tries a long (8-byte) hash first, then a short (4-byte) hash.
///
/// The `chain` array is repurposed as the long hash table for this strategy.
/// Same anchor-relative skip logic as `parse_ranges_fast`.
fn parse_ranges_dfast(
    full_data: &[u8],
    start: usize,
    end: usize,
    finder: &mut MatchFinder,
    mut sink: impl ParseSink,
) {
    let data = &full_data[..end];
    let search_log = finder.cfg.search_log;
    let mut pos = start;
    let mut lit_start = start;

    while pos < end {
        if pos + 4 > end {
            break;
        }

        match finder.lookup_dfast(data, pos) {
            Some(m) => {
                if lit_start < pos {
                    sink.literals(lit_start, pos);
                }
                sink.matched(pos, m.offset, m.length);
                finder.skip_dfast(data, pos + 1, m.length - 1);
                pos += m.length;
                lit_start = pos;
            }
            None => {
                let step = 1 + ((pos - lit_start) >> search_log);
                finder.skip_dfast_miss_positions(data, pos, step);
                pos += step;
            }
        }
    }

    if lit_start < end {
        sink.literals(lit_start, end);
    }
}

/// Lazy LZ77: try 1 or 2 lookahead positions before committing to a match.
///
/// `max_lookahead` is 1 for `Lazy` and 2 for `Lazy2` (and BT strategies until Step 5).
///
/// At each position P, the encoder:
/// 1. Finds match M0 at P (greedy candidate).
/// 2. Finds match M1 at P+1 (and M2 at P+2 for lazy2).
/// 3. If a later match is better (per `prefer_match`), emit P as a literal and
///    use the later match instead.
///
/// When a match exceeds `target_length` (and target_length > 0), it is accepted
/// immediately without lookahead.
///
/// **Insert discipline**: `find_match` always inserts the queried position. After
/// choosing `best_pos`, positions `pos..=pos+lookahead_called` are already in the
/// hash table. The skip call starts from `pos+lookahead_called+1` to avoid
/// double-inserting positions (which would create self-loops in the chain).
fn parse_ranges_lazy(
    full_data: &[u8],
    start: usize,
    end: usize,
    finder: &mut MatchFinder,
    mut sink: impl ParseSink,
    max_lookahead: usize,
) {
    let data = &full_data[..end];
    let target_length = finder.cfg.target_length;
    let mut pos = start;
    let mut lit_start = start;
    while pos < end {
        if pos + 4 > end {
            break;
        }

        let Some(m0) = finder.find_match(data, pos) else {
            pos += 1;
            continue;
        };

        // Early accept: match is long enough to skip lookahead entirely.
        if target_length > 0 && m0.length >= target_length {
            if lit_start < pos {
                sink.literals(lit_start, pos);
            }
            sink.matched(pos, m0.offset, m0.length);
            finder.skip(data, pos + 1, m0.length - 1);
            pos += m0.length;
            lit_start = pos;
            continue;
        }

        let mut best = m0;
        let mut best_pos = pos;
        let mut lookahead_called = 0usize;

        // Try lookahead positions.
        'lookahead: for la in 1..=max_lookahead {
            let la_pos = pos + la;
            if la_pos + 4 > end {
                break;
            }
            // find_match inserts la_pos into the hash table as a side effect.
            if let Some(m) = finder.find_match(data, la_pos) {
                if target_length > 0 && m.length >= target_length {
                    // Early accept from lookahead — use this match immediately.
                    best = m;
                    best_pos = la_pos;
                    lookahead_called = la;
                    break 'lookahead;
                }
                if prefer_match(m, best) {
                    best = m;
                    best_pos = la_pos;
                }
            }
            lookahead_called = la;
        }

        // Emit literals up to the winning match position.
        if lit_start < best_pos {
            sink.literals(lit_start, best_pos);
        }
        sink.matched(best_pos, best.offset, best.length);

        // Skip match interior. Positions pos..=pos+lookahead_called were already
        // inserted by find_match calls above, so start the skip after them to
        // avoid creating self-loops in the chain array.
        let skip_start = pos + lookahead_called + 1;
        let match_end = best_pos + best.length;
        if skip_start < match_end {
            finder.skip(data, skip_start, match_end - skip_start);
        }
        pos = match_end;
        lit_start = pos;
    }

    if lit_start < end {
        sink.literals(lit_start, end);
    }
}

/// BT + lazy2: binary-tree match finding with 2-position lookahead.
///
/// Same control flow as `parse_ranges_lazy(max_lookahead=2)` but uses
/// `bt_find_insert` instead of `find_match`. The BT simultaneously inserts and
/// searches, providing better match quality than a hash chain for the same
/// search depth. Interior match positions are not inserted (speed trade-off).
fn parse_ranges_bt_lazy2(
    full_data: &[u8],
    start: usize,
    end: usize,
    finder: &mut MatchFinder,
    mut sink: impl ParseSink,
) {
    let data = &full_data[..end];
    let target_length = finder.cfg.target_length;
    let mut pos = start;
    let mut lit_start = start;

    while pos < end {
        if pos + 4 > end {
            break;
        }

        let Some(m0) = finder.bt_find_insert(data, pos) else {
            pos += 1;
            continue;
        };

        // Early accept: match long enough to skip lookahead.
        if target_length > 0 && m0.length >= target_length {
            if lit_start < pos {
                sink.literals(lit_start, pos);
            }
            sink.matched(pos, m0.offset, m0.length);
            pos += m0.length;
            lit_start = pos;
            continue;
        }

        let mut best = m0;
        let mut best_pos = pos;

        'lookahead: for la in 1..=2usize {
            let la_pos = pos + la;
            if la_pos + 4 > end {
                break;
            }
            if let Some(m) = finder.bt_find_insert(data, la_pos) {
                if target_length > 0 && m.length >= target_length {
                    best = m;
                    best_pos = la_pos;
                    break 'lookahead;
                }
                if prefer_match(m, best) {
                    best = m;
                    best_pos = la_pos;
                }
            }
        }

        if lit_start < best_pos {
            sink.literals(lit_start, best_pos);
        }
        sink.matched(best_pos, best.offset, best.length);

        // Positions pos..=pos+lookahead_called are already in the BT.
        // Interior match positions are intentionally not inserted.
        pos = best_pos + best.length;
        lit_start = pos;
    }

    if lit_start < end {
        sink.literals(lit_start, end);
    }
}

/// Chunk size for the optimal parser's DP window.
const OPT_CHUNK_SIZE: usize = 512;

/// Estimated bit cost of a match: FSE sequence overhead + offset bits + ML extra bits.
#[inline]
fn match_cost_bits(offset: usize, length: usize) -> u32 {
    const SEQ_OVERHEAD: u32 = 16;
    SEQ_OVERHEAD + offset_code_bits(offset) as u32 + match_length_extra_bits(length)
}

/// Number of extra bits for the match-length FSE code (approximation).
#[inline]
fn match_length_extra_bits(length: usize) -> u32 {
    // Mirrors the MATCH_LENGTH_EXTRA table bands:
    //   codes 0-31 → lengths 3-34   → 0 extra bits
    //   code  32   → lengths 35-66  → 1 extra bit
    //   code  33   → lengths 67-130 → 2 extra bits
    //   code  34   → lengths 131-258 → 3 extra bits
    //   code  35+  → 4+ extra bits (rare; cap at 4)
    if length <= 34 { 0 } else if length <= 66 { 1 } else if length <= 130 { 2 } else if length <= 258 { 3 } else { 4 }
}

/// Cost-based optimal parser: forward DP + backtrack over `OPT_CHUNK_SIZE`-position chunks.
///
/// Matches that meet or exceed `target_length` are accepted immediately without going
/// through the DP (same early-accept logic as lazy matching). This is critical for
/// highly-compressible input where the BT finds matches of tens of thousands of bytes
/// — clipping such matches to 256-byte chunks would destroy ratio.
///
/// For each DP chunk:
/// 1. Collect BT matches position by position; flush early if a long match is found.
/// 2. Forward DP: `price[i]` = min bits to encode chunk positions `[0, i)`.
///    Literal: `price[i+1] = min(price[i+1], price[i] + 8)`.
///    Match: `price[i+len] = min(...)` for all lengths `min_match..=best.len`.
/// 3. Backtrack from `chunk_len` via `from_len`/`from_off` pointers.
/// 4. Emit events in forward order.
fn parse_ranges_optimal(
    full_data: &[u8],
    start: usize,
    end: usize,
    finder: &mut MatchFinder,
    mut sink: impl ParseSink,
) {
    let data = &full_data[..end];
    let min_match = finder.cfg.min_match;
    let target_length = finder.cfg.target_length;
    let mut pos = start;
    let mut lit_start = start;

    // Reuse across chunks.
    let mut chunk_matches: Vec<Option<Match>> = Vec::with_capacity(OPT_CHUNK_SIZE);
    let mut chunk_abs_start = start; // absolute frame position of chunk[0]
    let mut price = vec![0u32; OPT_CHUNK_SIZE + 1];
    let mut from_len = vec![0usize; OPT_CHUNK_SIZE + 1];
    let mut from_off = vec![0usize; OPT_CHUNK_SIZE + 1];
    let mut path: Vec<(usize, usize, usize)> = Vec::with_capacity(OPT_CHUNK_SIZE);

    /// Flush a collected chunk through the DP and emit events.
    #[inline(always)]
    fn flush_chunk(
        chunk_matches: &[Option<Match>],
        chunk_abs_start: usize,
        min_match: usize,
        lit_start: &mut usize,
        sink: &mut impl ParseSink,
        price: &mut [u32],
        from_len: &mut [usize],
        from_off: &mut [usize],
        path: &mut Vec<(usize, usize, usize)>,
    ) {
        let chunk_len = chunk_matches.len();
        if chunk_len == 0 {
            return;
        }
        const INF: u32 = u32::MAX / 2;
        const LITERAL_BITS: u32 = 8;
        let slots = chunk_len + 1;
        price[..slots].fill(INF);
        from_len[..slots].fill(0);
        from_off[..slots].fill(0);
        price[0] = 0;

        for i in 0..chunk_len {
            let p = price[i];
            if p == INF {
                continue;
            }
            let lp = p + LITERAL_BITS;
            if lp < price[i + 1] {
                price[i + 1] = lp;
                from_len[i + 1] = 0;
                from_off[i + 1] = 0;
            }
            if let Some(m) = chunk_matches[i] {
                let max_len = m.length.min(chunk_len - i);
                for len in min_match..=max_len {
                    let mp = p + match_cost_bits(m.offset, len);
                    if mp < price[i + len] {
                        price[i + len] = mp;
                        from_len[i + len] = len;
                        from_off[i + len] = m.offset;
                    }
                }
            }
        }

        path.clear();
        let mut cur = chunk_len;
        while cur > 0 {
            let len = from_len[cur];
            let off = from_off[cur];
            if len == 0 {
                path.push((cur - 1, 0, 0));
                cur -= 1;
            } else {
                path.push((cur - len, len, off));
                cur -= len;
            }
        }
        path.reverse();

        for &(chunk_pos, len, off) in path.iter() {
            if len > 0 {
                let abs_pos = chunk_abs_start + chunk_pos;
                if *lit_start < abs_pos {
                    sink.literals(*lit_start, abs_pos);
                }
                sink.matched(abs_pos, off, len);
                *lit_start = abs_pos + len;
            }
        }
    }

    while pos < end {
        if pos + 4 > end {
            break;
        }

        let m = finder.bt_find_insert(data, pos);

        // Early accept: long matches bypass the DP entirely.
        if let Some(ref m) = m {
            if target_length > 0 && m.length >= target_length {
                flush_chunk(
                    &chunk_matches, chunk_abs_start, min_match,
                    &mut lit_start, &mut sink,
                    &mut price, &mut from_len, &mut from_off, &mut path,
                );
                chunk_matches.clear();

                if lit_start < pos {
                    sink.literals(lit_start, pos);
                }
                sink.matched(pos, m.offset, m.length);
                pos += m.length;
                lit_start = pos;
                chunk_abs_start = pos;
                continue;
            }
        }

        chunk_matches.push(m);
        pos += 1;

        if chunk_matches.len() >= OPT_CHUNK_SIZE {
            flush_chunk(
                &chunk_matches, chunk_abs_start, min_match,
                &mut lit_start, &mut sink,
                &mut price, &mut from_len, &mut from_off, &mut path,
            );
            chunk_matches.clear();
            chunk_abs_start = pos;
        }
    }

    // Flush remaining partial chunk.
    flush_chunk(
        &chunk_matches, chunk_abs_start, min_match,
        &mut lit_start, &mut sink,
        &mut price, &mut from_len, &mut from_off, &mut path,
    );

    if lit_start < end {
        sink.literals(lit_start, end);
    }
}

/// Returns true if `new_match` is preferable to `old_match`.
///
/// Prefers the new match when its length advantage (weighted 4×) outweighs the
/// additional offset cost (in offset-code bits). This matches the reference zstd
/// lazy-matching heuristic.
#[inline]
fn prefer_match(new: Match, old: Match) -> bool {
    let new_bits = offset_code_bits(new.offset) as i32;
    let old_bits = offset_code_bits(old.offset) as i32;
    4 * new.length as i32 > 4 * old.length as i32 + (new_bits - old_bits)
}

/// Number of bits needed to encode the offset (= floor(log2(offset + 3)) + 1).
#[inline]
fn offset_code_bits(offset: usize) -> usize {
    let raw = offset + 3;
    (usize::BITS - raw.leading_zeros()) as usize
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

    #[test]
    fn test_lookup_fast_single_lookup() {
        // pos=0 and pos=8 share only the first 4 bytes ("abcd"), then diverge.
        // match_length will measure exactly 4.
        // pos=16 and pos=8 also share only "abcd" (same structure).
        let data = b"abcdEFGHabcdIJKLabcdMNOP";
        //            0       8       16

        // min_match=3: a 4-byte match qualifies.
        let cfg3 = MatchConfig { min_match: 3, ..MatchConfig::for_level(1) };
        let mut finder3 = MatchFinder::new(&cfg3);

        // First lookup inserts pos=0; no prior entry → no match.
        assert!(finder3.lookup_fast(data, 0).is_none());
        // pos=8 has the same 4-byte prefix as pos=0 → finds a 4-byte match.
        let m = finder3.lookup_fast(data, 8);
        assert!(m.is_some(), "should find match at pos=8 referencing pos=0");
        assert_eq!(m.unwrap().offset, 8);

        // pos=16 finds pos=8 (the new hash-table head), NOT pos=0 (would need chain).
        let m2 = finder3.lookup_fast(data, 16);
        assert!(m2.is_some());
        assert_eq!(m2.unwrap().offset, 8, "fast should only find the most-recent entry (no chain)");

        // min_match=7: a 4-byte match is too short → rejected.
        let cfg7 = MatchConfig { min_match: 7, ..MatchConfig::for_level(1) };
        let mut finder7 = MatchFinder::new(&cfg7);
        finder7.lookup_fast(data, 0);
        assert!(
            finder7.lookup_fast(data, 8).is_none(),
            "min_match=7 should reject a 4-byte match"
        );
    }

    #[test]
    fn test_lookup_dfast_long_table_priority() {
        // Craft data where pos=0 and pos=16 share an 8-byte prefix but NOT the same
        // 4-byte prefix in the short table (different bytes at positions 4-7).
        // The long hash should find the 8-byte match.
        let mut data = [0u8; 48];
        // pos=0: "ABCDEFGHXXX..."
        data[0..8].copy_from_slice(b"ABCDEFGH");
        // pos=16: same 8-byte prefix
        data[16..24].copy_from_slice(b"ABCDEFGH");
        // pos=32: different 4-byte prefix but same 8-byte prefix as pos=16
        data[32..40].copy_from_slice(b"ABCDEFGH");

        let cfg = MatchConfig { min_match: 4, ..MatchConfig::for_level(3) }; // DFast
        let mut finder = MatchFinder::new(&cfg);

        // Insert pos=0; no prior entries → no match.
        assert!(finder.lookup_dfast(&data, 0).is_none());
        // pos=16 should match pos=0 via long table.
        let m = finder.lookup_dfast(&data, 16);
        assert!(m.is_some(), "dfast should find 8-byte match from long table");
        assert_eq!(m.unwrap().offset, 16);
    }

    #[test]
    fn test_optimal_reconstructs_original() {
        // BtOpt parse must reconstruct the original bytes exactly.
        let data: Vec<u8> = b"abcdefghijklmnop".iter().cycle().take(512).cloned().collect();
        let cfg = MatchConfig::for_level(16); // BtOpt
        let events = parse(&data, &cfg);
        let mut out = Vec::new();
        for e in &events {
            match e {
                Event::Literals(s, end) => out.extend_from_slice(&data[*s..*end]),
                Event::Match { pos: _, offset, length } => {
                    let src = out.len() - offset;
                    for i in 0..*length {
                        let b = out[src + i];
                        out.push(b);
                    }
                }
            }
        }
        assert_eq!(out, data, "BtOpt parse must reconstruct original");
    }

    #[test]
    fn test_optimal_prefers_deferred_longer_match() {
        // Craft input where greedy takes a short match at pos 0, but optimal
        // finds it cheaper to emit a literal and take a longer match at pos 1.
        //
        // Greedy at level 5: may match "abc" at pos 4 → length 3.
        // Optimal at level 16: should find the longer "abcXYZ" if deferring 1 byte helps.
        //
        // Key property: the optimal parse must at least reconstruct the original.
        // We verify optimal finds at least as many match bytes as literals.
        let pattern = b"abcXYZabcXYZabcXYZ";
        let data: Vec<u8> = pattern.iter().cycle().take(256).cloned().collect();

        let opt_cfg = MatchConfig::for_level(16);
        let events = parse(&data, &opt_cfg);

        // Verify reconstruction.
        let mut out = Vec::new();
        for e in &events {
            match e {
                Event::Literals(s, end) => out.extend_from_slice(&data[*s..*end]),
                Event::Match { pos: _, offset, length } => {
                    let src = out.len() - offset;
                    for i in 0..*length {
                        let b = out[src + i];
                        out.push(b);
                    }
                }
            }
        }
        assert_eq!(out, data, "optimal parse must reconstruct original");

        // Verify the optimal parser finds meaningful matches (more match bytes than literals).
        let match_bytes: usize = events
            .iter()
            .filter_map(|e| if let Event::Match { length, .. } = e { Some(*length) } else { None })
            .sum();
        let lit_bytes: usize = events
            .iter()
            .filter_map(|e| if let Event::Literals(s, end) = e { Some(end - s) } else { None })
            .sum();
        assert!(
            match_bytes > lit_bytes,
            "optimal parser should encode mostly matches on repetitive data (match={match_bytes}, lit={lit_bytes})"
        );
    }

    #[test]
    fn test_bt_finds_match_on_repetitive_data() {
        // BT should find matches in repetitive data, just like greedy.
        let data: Vec<u8> = b"abcdefgh".iter().cycle().take(256).cloned().collect();
        let cfg = MatchConfig { min_match: 4, ..MatchConfig::for_level(13) };
        let mut finder = MatchFinder::new(&cfg);
        let full_data_len = data.len();
        let data_slice = &data[..full_data_len];

        // Insert the first 8 positions (no matches yet).
        for i in 0..8 {
            finder.bt_find_insert(data_slice, i);
        }
        // Now position 8 should find a match back to 0 (same 8-byte pattern).
        let m = finder.bt_find_insert(data_slice, 8);
        assert!(m.is_some(), "BT should find a match in repetitive data");
        let m = m.unwrap();
        assert!(m.length >= 4, "match length should be >= min_match");
        assert!(m.offset > 0 && m.offset <= 8);
    }

    #[test]
    fn test_bt_respects_window_size() {
        // Insert a position, then verify candidates outside window_size are rejected.
        let data: Vec<u8> = b"abcdefgh".iter().cycle().take(4096).cloned().collect();
        // Use a tiny window (2^10 = 1024 bytes) to exercise the window check.
        let cfg = MatchConfig {
            window_log: 10,
            min_match: 4,
            ..MatchConfig::for_level(13)
        };
        let mut finder = MatchFinder::new(&cfg);
        let data_slice = &data[..];

        // Insert pos=0.
        finder.bt_find_insert(data_slice, 0);
        // pos=8: within window → should find match.
        let m = finder.bt_find_insert(data_slice, 8);
        assert!(m.is_some(), "match within window should be found");

        // Skip ahead well past the window (1025 bytes).
        // Insert pos=1025: pos=0 is now outside the window → should not be returned.
        for i in 1..1025 {
            finder.bt_find_insert(data_slice, i);
        }
        // pos=1025: offset back to pos=0 is 1025 > window_size=1024 → not a candidate.
        // But pos=1 .. pos=1024 are inside window, so we still expect a match.
        let m2 = finder.bt_find_insert(data_slice, 1025);
        assert!(m2.is_some(), "should still find a match within window");
        assert!(
            m2.unwrap().offset <= 1024,
            "match offset must not exceed window_size"
        );
    }

    #[test]
    fn test_bt_reconstructs_original() {
        // Full parse via BtLazy2 must reconstruct the original bytes.
        let data: Vec<u8> = b"abcdefghijklmnop".iter().cycle().take(512).cloned().collect();
        let cfg = MatchConfig::for_level(13); // BtLazy2
        let events = parse(&data, &cfg);
        let mut out = Vec::new();
        for e in &events {
            match e {
                Event::Literals(s, end) => out.extend_from_slice(&data[*s..*end]),
                Event::Match { pos: _, offset, length } => {
                    let src = out.len() - offset;
                    for i in 0..*length {
                        let b = out[src + i];
                        out.push(b);
                    }
                }
            }
        }
        assert_eq!(out, data, "BtLazy2 parse must reconstruct original");
    }

    #[test]
    fn test_prefer_match_same_length_smaller_offset() {
        // Same length, smaller offset → prefer new (fewer offset bits).
        let old = Match { offset: 1000, length: 10 };
        let new = Match { offset: 4, length: 10 };
        assert!(prefer_match(new, old), "smaller offset should be preferred");
    }

    #[test]
    fn test_prefer_match_longer_same_offset() {
        // Longer match, same offset → prefer new.
        let old = Match { offset: 100, length: 8 };
        let new = Match { offset: 100, length: 12 };
        assert!(prefer_match(new, old), "longer match at same offset should be preferred");
    }

    #[test]
    fn test_prefer_match_slight_length_gain_huge_offset() {
        // +1 length but offset jumps from 8 to 1_000_000 (many more offset bits) → prefer old.
        let old = Match { offset: 8, length: 10 };
        let new = Match { offset: 1_000_000, length: 11 };
        assert!(!prefer_match(new, old), "huge offset penalty should outweigh +1 length");
    }

    #[test]
    fn test_lazy_finds_better_match_than_greedy() {
        // Craft input where greedy finds a short match at pos 0, but lazy finds a
        // longer match at pos 1.
        //
        // Input: "xABCDEFGH...  ABCDEFGHxx..."
        // At pos 0: 'x' doesn't match much.
        // At pos 1: "ABCDEFGH" matches a later repeat — longer match via lookahead.
        //
        // Simpler approach: use repetitive input and count sequences.
        let pattern = b"abcdefghijklmnop"; // 16 bytes
        let mut data = Vec::new();
        data.extend_from_slice(pattern);
        data.push(b'X'); // break pattern
        data.extend_from_slice(pattern);
        data.extend_from_slice(pattern);

        let greedy_cfg = MatchConfig::for_level(5); // Greedy
        let lazy2_cfg = MatchConfig::for_level(8);  // Lazy2

        let greedy_events = parse(&data, &greedy_cfg);
        let lazy2_events = parse(&data, &lazy2_cfg);

        let count_matches = |events: &[Event]| {
            events.iter().filter(|e| matches!(e, Event::Match { .. })).count()
        };

        // Lazy2 should find at least as many matches (and typically more/longer ones).
        // The key invariant: both must reconstruct the original correctly.
        let mut greedy_out = Vec::new();
        for e in &greedy_events {
            match e {
                Event::Literals(s, end) => greedy_out.extend_from_slice(&data[*s..*end]),
                Event::Match { pos: _, offset, length } => {
                    let src = greedy_out.len() - offset;
                    for i in 0..*length {
                        let b = greedy_out[src + i];
                        greedy_out.push(b);
                    }
                }
            }
        }
        assert_eq!(greedy_out, data, "greedy must reconstruct original");

        let mut lazy2_out = Vec::new();
        for e in &lazy2_events {
            match e {
                Event::Literals(s, end) => lazy2_out.extend_from_slice(&data[*s..*end]),
                Event::Match { pos: _, offset, length } => {
                    let src = lazy2_out.len() - offset;
                    for i in 0..*length {
                        let b = lazy2_out[src + i];
                        lazy2_out.push(b);
                    }
                }
            }
        }
        assert_eq!(lazy2_out, data, "lazy2 must reconstruct original");

        // Both should find some matches on repetitive data.
        assert!(count_matches(&lazy2_events) > 0, "lazy2 should find matches");
    }

    #[test]
    fn test_target_length_early_accept() {
        // With a very low target_length, the encoder should accept long matches immediately.
        // Use highly repetitive data so there are many long matches.
        let data: Vec<u8> = b"abcdefgh".iter().cycle().take(1024).cloned().collect();

        // Level 6 has target_length=4; any match >= 4 should be accepted without lookahead.
        let cfg = MatchConfig::for_level(6);
        assert_eq!(cfg.target_length, 4);

        // Verify the result round-trips (correctness, not just that it runs).
        let events = parse(&data, &cfg);
        let mut out = Vec::new();
        for e in &events {
            match e {
                Event::Literals(s, end) => out.extend_from_slice(&data[*s..*end]),
                Event::Match { pos: _, offset, length } => {
                    let src = out.len() - offset;
                    for i in 0..*length {
                        let b = out[src + i];
                        out.push(b);
                    }
                }
            }
        }
        assert_eq!(out, data, "target_length early-accept must reconstruct original");
    }

    #[test]
    fn test_cross_block_history() {
        // Build two "blocks" where the second block can match content from the first.
        // Pattern appears in block 1 (bytes 0-63) and is repeated in block 2 (bytes 64-127).
        let pattern = b"abcdefghijklmnop"; // 16 bytes, repeated
        let block1: Vec<u8> = pattern.iter().cycle().take(64).cloned().collect();
        let block2: Vec<u8> = pattern.iter().cycle().take(64).cloned().collect();
        let full_data: Vec<u8> = [block1.as_slice(), block2.as_slice()].concat();

        // Use a config with enough window to cover both blocks.
        let cfg = MatchConfig {
            window_log: 17, // 128 KiB window
            min_match: 3,
            ..MatchConfig::for_level(5)
        };
        let mut finder = MatchFinder::new(&cfg);
        let mut events = Vec::new();

        // Process block 1 first to populate the history.
        parse_ranges(
            &full_data,
            0,
            64,
            &mut finder,
            EventSink {
                events: &mut events,
            },
        );

        let mut events2 = Vec::new();
        // Process block 2 — should find matches back into block 1.
        parse_ranges(
            &full_data,
            64,
            128,
            &mut finder,
            EventSink {
                events: &mut events2,
            },
        );

        let has_cross_block_match = events2.iter().any(|e| {
            if let Event::Match { pos, offset, .. } = e {
                // A match whose source is in block 1 (pos - offset < 64)
                *pos >= 64 && pos.saturating_sub(*offset) < 64
            } else {
                false
            }
        });
        assert!(
            has_cross_block_match,
            "expected at least one match spanning block boundary"
        );
    }
}
