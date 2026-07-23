#![no_main]

use libfuzzer_sys::fuzz_target;
use phoenix_crypto::{BlindIndexKey, BlindIndexer};

fuzz_target!(|data: &[u8]| {
    let key = BlindIndexKey::new("fuzz-v1", [0x24; 32]).expect("fuzz key is valid");
    let indexer = BlindIndexer::new(key);
    let envelope = String::from_utf8_lossy(data);
    let _ = indexer.verify(&envelope, "fuzz.user.email", data);
});
