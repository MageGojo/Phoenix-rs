use phoenix::prelude::{Routes, typed};

use crate::module_home;

#[must_use]
pub fn routes() -> Routes {
    Routes::new()
        .get("/", typed(module_home))
        .name("dashboard")
        .get("/users", typed(module_home))
        .name("users.index")
}
