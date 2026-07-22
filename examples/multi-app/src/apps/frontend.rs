use phoenix::prelude::{Routes, typed};

use crate::module_home;

#[must_use]
pub fn routes() -> Routes {
    Routes::new()
        .get("/", typed(module_home))
        .name("home")
        .get("/account", typed(module_home))
        .name("account")
}
