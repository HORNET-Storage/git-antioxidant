//! The order in which git visits the entries of its string-keyed hash maps.

/// The number of buckets of a new git `hashmap`, `HASHMAP_INITIAL_SIZE`.
const INITIAL_BUCKETS: usize = 64;

/// Return the indices of `keys` in the order in which git's `strmap`, `strintmap` or `strset` visits them, after the
/// keys were added to an empty map in the order of `keys`, each once, and none was removed.
///
/// Git hashes a key with the 32-bit FNV-1 hash of its bytes (`strhash()`). Its `hashmap` starts with 64 buckets, of
/// which a key takes the one its hash selects, and quadruples them once it holds more than 80% as many entries. A new
/// entry goes to the head of its bucket's chain; growing moves the entries bucket by bucket, each chain from its head,
/// to the heads of their new buckets' chains; and iteration visits the buckets in order and each chain from its head.
/// Merges in git depend on this order where candidates tie, for example among the directories to which the files of a
/// removed directory went equally often.
pub fn iteration_order<K: AsRef<[u8]>>(keys: impl IntoIterator<Item = K>) -> Vec<usize> {
    let hashes: Vec<u32> = keys.into_iter().map(|key| strhash(key.as_ref())).collect();
    let mut heads: Vec<Option<usize>> = vec![None; INITIAL_BUCKETS];
    let mut next: Vec<Option<usize>> = vec![None; hashes.len()];
    for (entry, &hash) in hashes.iter().enumerate() {
        let chosen = bucket(hash, heads.len());
        next[entry] = heads[chosen].replace(entry);
        if entry + 1 > heads.len() * 80 / 100 {
            let mut grown = vec![None; heads.len() * 4];
            for head in heads {
                let mut moving = head;
                while let Some(moved) = moving {
                    moving = next[moved];
                    let chosen = bucket(hashes[moved], grown.len());
                    next[moved] = grown[chosen].replace(moved);
                }
            }
            heads = grown;
        }
    }

    let mut order = Vec::with_capacity(hashes.len());
    for head in heads {
        let mut visiting = head;
        while let Some(entry) = visiting {
            order.push(entry);
            visiting = next[entry];
        }
    }
    order
}

/// Git's `strhash()`: the 32-bit FNV-1 hash of `key`, whose bytes, like those of a string of git, aren't NUL.
fn strhash(key: &[u8]) -> u32 {
    key.iter().fold(0x811c_9dc5_u32, |hash, &byte| {
        hash.wrapping_mul(0x0100_0193) ^ u32::from(byte)
    })
}

/// The bucket among `buckets`, a power of two, that a key of `hash` takes.
fn bucket(hash: u32, buckets: usize) -> usize {
    hash as usize & (buckets - 1)
}
