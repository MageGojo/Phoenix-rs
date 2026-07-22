use phoenix::prelude::{Routes, routes, typed};

use crate::module_home;

#[must_use]
pub fn routes() -> Routes {
    routes! {
        GET "/" => typed(module_home), name = "dashboard";
        GET "/users" => typed(module_home), name = "users.index";
    }
}
