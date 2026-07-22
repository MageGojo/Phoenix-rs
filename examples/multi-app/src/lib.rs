use phoenix::prelude::*;

pub mod apps;

#[derive(Clone, Debug)]
pub struct AppBrand(pub &'static str);

/// Compile the website, customer frontend, and admin into one server.
///
/// # Errors
///
/// Returns an error when an application selector or route declaration is invalid.
pub fn application() -> Result<Application, MultiApplicationError> {
    applications! {
        website => apps::website::routes(), [root, state = AppBrand("Official website")];
        frontend => apps::frontend::routes(), [prefix = "/app", state = AppBrand("Customer frontend")];
        admin => apps::admin::routes(), [state = AppBrand("Administration")];
    }
}

pub async fn module_home(
    State(brand): State<AppBrand>,
    State(application): State<ApplicationContext>,
) -> String {
    format!("{} [{}]", brand.0, application.name())
}
