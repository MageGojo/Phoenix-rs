#![no_main]

use libfuzzer_sys::fuzz_target;
use phoenix_http::{CspNonce, Redirect};

fuzz_target!(|data: &[u8]| {
    let input = String::from_utf8_lossy(data);
    let _ = CspNonce::new(input.as_ref());
    let _ = Redirect::see_other(input.as_ref());
});
